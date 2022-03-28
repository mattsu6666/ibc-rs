//! ICS4 (channel) context. The two traits `ChannelReader ` and `ChannelKeeper` define
//! the interface that any host chain must implement to be able to process any `ChannelMsg`.
//!
use core::time::Duration;
use num_traits::float::FloatCore;

use crate::core::ics02_client::client_consensus::AnyConsensusState;
use crate::core::ics02_client::client_state::AnyClientState;
use crate::core::ics03_connection::connection::ConnectionEnd;
use crate::core::ics04_channel::channel::ChannelEnd;
use crate::core::ics04_channel::handler::recv_packet::RecvPacketResult;
use crate::core::ics04_channel::handler::{ChannelIdState, ChannelResult};
use crate::core::ics04_channel::{error::Error, packet::Receipt};
use crate::core::ics05_port::capabilities::{CapabilityName, ChannelCapability};
use crate::core::ics05_port::context::CapabilityReader;
use crate::core::ics24_host::identifier::{ChannelId, ClientId, ConnectionId, PortId};
use crate::core::ics24_host::path::ChannelCapabilityPath;
use crate::core::ics26_routing::context::ModuleId;
use crate::prelude::*;
use crate::timestamp::Timestamp;
use crate::Height;

use super::packet::{PacketResult, Sequence};

/// A context supplying all the necessary read-only dependencies for processing any `ChannelMsg`.
pub trait ChannelReader: CapabilityReader {
    /// Returns the ChannelEnd for the given `port_id` and `chan_id`.
    fn channel_end(&self, port_channel_id: &(PortId, ChannelId)) -> Result<ChannelEnd, Error>;

    /// Returns the ConnectionState for the given identifier `connection_id`.
    fn connection_end(&self, connection_id: &ConnectionId) -> Result<ConnectionEnd, Error>;

    fn connection_channels(&self, cid: &ConnectionId) -> Result<Vec<(PortId, ChannelId)>, Error>;

    /// Returns the ClientState for the given identifier `client_id`. Necessary dependency towards
    /// proof verification.
    fn client_state(&self, client_id: &ClientId) -> Result<AnyClientState, Error>;

    fn client_consensus_state(
        &self,
        client_id: &ClientId,
        height: Height,
    ) -> Result<AnyConsensusState, Error>;

    fn authenticated_capability(&self, port_id: &PortId) -> Result<ChannelCapability, Error>;

    fn get_next_sequence_send(
        &self,
        port_channel_id: &(PortId, ChannelId),
    ) -> Result<Sequence, Error>;

    fn get_next_sequence_recv(
        &self,
        port_channel_id: &(PortId, ChannelId),
    ) -> Result<Sequence, Error>;

    fn get_next_sequence_ack(
        &self,
        port_channel_id: &(PortId, ChannelId),
    ) -> Result<Sequence, Error>;

    fn get_packet_commitment(&self, key: &(PortId, ChannelId, Sequence)) -> Result<String, Error>;

    fn get_packet_receipt(&self, key: &(PortId, ChannelId, Sequence)) -> Result<Receipt, Error>;

    fn get_packet_acknowledgement(
        &self,
        key: &(PortId, ChannelId, Sequence),
    ) -> Result<String, Error>;

    /// A hashing function for packet commitments
    fn hash(&self, value: String) -> String;

    /// Returns the current height of the local chain.
    fn host_height(&self) -> Height;

    /// Returns the current timestamp of the local chain.
    fn host_timestamp(&self) -> Timestamp {
        let pending_consensus_state = self
            .pending_host_consensus_state()
            .expect("host must have pending consensus state");
        pending_consensus_state.timestamp()
    }

    /// Returns the `ConsensusState` of the host (local) chain at a specific height.
    fn host_consensus_state(&self, height: Height) -> Result<AnyConsensusState, Error>;

    /// Returns the pending `ConsensusState` of the host (local) chain.
    fn pending_host_consensus_state(&self) -> Result<AnyConsensusState, Error>;

    /// Returns the time when the client state for the given [`ClientId`] was updated with a header for the given [`Height`]
    fn client_update_time(&self, client_id: &ClientId, height: Height) -> Result<Timestamp, Error>;

    /// Returns the height when the client state for the given [`ClientId`] was updated with a header for the given [`Height`]
    fn client_update_height(&self, client_id: &ClientId, height: Height) -> Result<Height, Error>;

    /// Returns a counter on the number of channel ids have been created thus far.
    /// The value of this counter should increase only via method
    /// `ChannelKeeper::increase_channel_counter`.
    fn channel_counter(&self) -> Result<u64, Error>;

    /// Returns the maximum expected time per block
    fn max_expected_time_per_block(&self) -> Duration;

    /// Calculates the block delay period using the connection's delay period and the maximum
    /// expected time per block.
    fn block_delay(&self, delay_period_time: Duration) -> u64 {
        calculate_block_delay(delay_period_time, self.max_expected_time_per_block())
    }

    /// Return the module_id along with the capability associated with a given (channel-id, port_id)
    fn lookup_module_by_channel(
        &self,
        channel_id: &ChannelId,
        port_id: &PortId,
    ) -> Result<(ModuleId, ChannelCapability), Error>;

    /// Check if the specified port_id is already bounded
    fn get_channel_capability(
        &self,
        port_id: PortId,
        channel_id: ChannelId,
    ) -> Result<ChannelCapability, Error> {
        CapabilityReader::get_capability(self, &self.channel_capability_name(port_id, channel_id))
            .map(Into::into)
            .map_err(Error::ics05_port)
    }

    /// Authenticate a capability key against a port_id by checking if the capability was previously
    /// generated and bound to the specified port
    fn authenticate(
        &self,
        port_id: PortId,
        channel_id: ChannelId,
        capability: &ChannelCapability,
    ) -> bool {
        self.authenticate_capability(
            &self.channel_capability_name(port_id, channel_id),
            capability,
        )
        .is_ok()
    }

    fn channel_capability_name(&self, port_id: PortId, channel_id: ChannelId) -> CapabilityName {
        ChannelCapabilityPath(port_id, channel_id)
            .to_string()
            .parse()
            .expect("ChannelEndsPath cannot be empty string")
    }
}

/// A context supplying all the necessary write-only dependencies (i.e., storage writing facility)
/// for processing any `ChannelMsg`.
pub trait ChannelKeeper {
    fn store_channel_result(&mut self, result: ChannelResult) -> Result<(), Error> {
        // The handler processed this channel & some modifications occurred, store the new end.
        self.store_channel(
            (result.port_id.clone(), result.channel_id.clone()),
            &result.channel_end,
        )?;

        // The channel identifier was freshly brewed.
        // Increase counter & initialize seq. nrs.
        if matches!(result.channel_id_state, ChannelIdState::Generated) {
            self.increase_channel_counter();

            // Associate also the channel end to its connection.
            self.store_connection_channels(
                result.channel_end.connection_hops()[0].clone(),
                &(result.port_id.clone(), result.channel_id.clone()),
            )?;

            // Initialize send, recv, and ack sequence numbers.
            self.store_next_sequence_send(
                (result.port_id.clone(), result.channel_id.clone()),
                1.into(),
            )?;
            self.store_next_sequence_recv(
                (result.port_id.clone(), result.channel_id.clone()),
                1.into(),
            )?;
            self.store_next_sequence_ack((result.port_id, result.channel_id), 1.into())?;
        }

        Ok(())
    }

    fn store_packet_result(&mut self, general_result: PacketResult) -> Result<(), Error> {
        match general_result {
            PacketResult::Send(res) => {
                self.store_next_sequence_send(
                    (res.port_id.clone(), res.channel_id.clone()),
                    res.seq_number,
                )?;

                self.store_packet_commitment(
                    (res.port_id.clone(), res.channel_id.clone(), res.seq),
                    res.timeout_timestamp,
                    res.timeout_height,
                    res.data,
                )?;
            }
            PacketResult::Recv(res) => {
                let res = match res {
                    RecvPacketResult::Success(res) => res,
                    RecvPacketResult::NoOp => unreachable!(),
                };
                match res.receipt {
                    None => {
                        // Ordered channel
                        self.store_next_sequence_recv(
                            (res.port_id.clone(), res.channel_id.clone()),
                            res.seq_number,
                        )?
                    }
                    Some(r) => {
                        // Unordered channel
                        self.store_packet_receipt(
                            (res.port_id.clone(), res.channel_id.clone(), res.seq),
                            r,
                        )?
                    }
                }
            }
            PacketResult::WriteAck(res) => {
                self.store_packet_acknowledgement(
                    (res.port_id.clone(), res.channel_id.clone(), res.seq),
                    res.ack,
                )?;
            }
            PacketResult::Ack(res) => {
                match res.seq_number {
                    Some(s) => {
                        //Ordered Channel
                        self.store_next_sequence_ack((res.port_id.clone(), res.channel_id), s)?;
                    }
                    None => {
                        //Unordered Channel
                        self.delete_packet_commitment((
                            res.port_id.clone(),
                            res.channel_id.clone(),
                            res.seq,
                        ))?;
                    }
                }
            }
            PacketResult::Timeout(res) => {
                if let Some(c) = res.channel {
                    //Ordered Channel
                    self.store_channel((res.port_id.clone(), res.channel_id.clone()), &c)?;
                }
                self.delete_packet_commitment((
                    res.port_id.clone(),
                    res.channel_id.clone(),
                    res.seq,
                ))?;
            }
        }
        Ok(())
    }

    fn store_packet_commitment(
        &mut self,
        key: (PortId, ChannelId, Sequence),
        timestamp: Timestamp,
        heigh: Height,
        data: Vec<u8>,
    ) -> Result<(), Error>;

    fn delete_packet_commitment(&mut self, key: (PortId, ChannelId, Sequence))
        -> Result<(), Error>;

    fn store_packet_receipt(
        &mut self,
        key: (PortId, ChannelId, Sequence),
        receipt: Receipt,
    ) -> Result<(), Error>;

    fn store_packet_acknowledgement(
        &mut self,
        key: (PortId, ChannelId, Sequence),
        ack: Vec<u8>,
    ) -> Result<(), Error>;

    fn delete_packet_acknowledgement(
        &mut self,
        key: (PortId, ChannelId, Sequence),
    ) -> Result<(), Error>;

    fn store_connection_channels(
        &mut self,
        conn_id: ConnectionId,
        port_channel_id: &(PortId, ChannelId),
    ) -> Result<(), Error>;

    /// Stores the given channel_end at a path associated with the port_id and channel_id.
    fn store_channel(
        &mut self,
        port_channel_id: (PortId, ChannelId),
        channel_end: &ChannelEnd,
    ) -> Result<(), Error>;

    fn store_next_sequence_send(
        &mut self,
        port_channel_id: (PortId, ChannelId),
        seq: Sequence,
    ) -> Result<(), Error>;

    fn store_next_sequence_recv(
        &mut self,
        port_channel_id: (PortId, ChannelId),
        seq: Sequence,
    ) -> Result<(), Error>;

    fn store_next_sequence_ack(
        &mut self,
        port_channel_id: (PortId, ChannelId),
        seq: Sequence,
    ) -> Result<(), Error>;

    /// Called upon channel identifier creation (Init or Try message processing).
    /// Increases the counter which keeps track of how many channels have been created.
    /// Should never fail.
    fn increase_channel_counter(&mut self);
}

pub fn calculate_block_delay(
    delay_period_time: Duration,
    max_expected_time_per_block: Duration,
) -> u64 {
    if max_expected_time_per_block.is_zero() {
        return 0;
    }

    FloatCore::ceil(delay_period_time.as_secs_f64() / max_expected_time_per_block.as_secs_f64())
        as u64
}

use std::collections::HashMap;

use ys_agent_core::OperationId;

use super::RouteKey;

/// Independent asynchronous lanes used by the TUI. Read lanes may run concurrently, while the
/// Provider lane represents durable or remotely visible mutations and is strictly single-flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AsyncChannel {
    DisplayContext,
    Catalog,
    Artifact,
    ProviderMutation,
    DatasourceMutation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsyncOperationTicket {
    pub operation_id: OperationId,
    pub channel: AsyncChannel,
    pub route_key: RouteKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsyncOperationBusy;

/// Pure result-admission state. A completion must match both the latest operation in its lane and
/// the currently visited route before a reducer is allowed to consume its payload.
#[derive(Debug, Default)]
pub struct AsyncResultGuard {
    active: HashMap<AsyncChannel, AsyncOperationTicket>,
}

impl AsyncResultGuard {
    pub fn start(
        &mut self,
        channel: AsyncChannel,
        route_key: RouteKey,
    ) -> Result<AsyncOperationTicket, AsyncOperationBusy> {
        if matches!(
            channel,
            AsyncChannel::ProviderMutation | AsyncChannel::DatasourceMutation
        ) && self.active.contains_key(&channel)
        {
            return Err(AsyncOperationBusy);
        }
        let ticket = AsyncOperationTicket {
            operation_id: OperationId::new(),
            channel,
            route_key,
        };
        self.active.insert(channel, ticket);
        Ok(ticket)
    }

    pub fn accept_completion(
        &mut self,
        ticket: AsyncOperationTicket,
        current_route: RouteKey,
    ) -> bool {
        if self.active.get(&ticket.channel) != Some(&ticket) {
            return false;
        }
        self.active.remove(&ticket.channel);
        ticket.route_key == current_route
    }

    pub fn cancel(&mut self, ticket: AsyncOperationTicket) -> bool {
        if self.active.get(&ticket.channel) != Some(&ticket) {
            return false;
        }
        self.active.remove(&ticket.channel);
        true
    }

    pub fn active(&self, channel: AsyncChannel) -> Option<AsyncOperationTicket> {
        self.active.get(&channel).copied()
    }
}

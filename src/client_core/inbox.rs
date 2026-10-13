//! The answers of the session in the order they were written (ibx#532).
//!
//! The reference gives its client the messages of one connection, in the
//! order they came. ibx keeps its answers in a queue per kind; each answer
//! takes the next count of the session's answer clock when it is written
//! (`bridge::SeqQueue`). A client takes every queue into its [`Inbox`] and
//! gives the answers in the order of their counts: a run of one kind at a
//! time, up to the first answer of another kind written after it.
//!
//! An answer is given only when every answer written before it is in the
//! inbox: the clock is read before the queues are taken, and an answer
//! with a count at or above it waits for the next pass (another one with a
//! smaller count may still be on its way into its queue).

use std::collections::VecDeque;

use crate::bridge::{FillExec, OrderReport, SharedState};
use crate::control::contracts::{ContractDefinition, SymbolMatch};
use crate::control::histogram::HistogramEntry;
use crate::control::historical::{HeadTimestampResponse, HistoricalBar, HistoricalResponse};
use crate::control::news::NewsHeadline;
use crate::control::scanner::ScannerResult;
use crate::types::*;

macro_rules! answer_kinds {
    ($($kind:ident: $ty:ty = $($path:ident).+;)*) => {
        /// The kinds of answers, one per queue.
        #[allow(non_camel_case_types)]
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub(crate) enum Kind { $($kind),* }

        /// The answers taken from the queues and not given yet, with their
        /// counts.
        #[derive(Default)]
        pub(crate) struct Inbox {
            $(pub(crate) $kind: VecDeque<(u64, $ty)>,)*
        }

        impl Inbox {
            /// Take every queue of the session.
            fn take_queues(&mut self, shared: &SharedState) {
                $(shared.$($path).+.drain_into(&mut self.$kind);)*
            }

            /// Put back the answers of `other`, each kind in the order of
            /// its counts. When this one is empty (no pass ran meanwhile),
            /// `other` takes its place with its buffers.
            pub(crate) fn merge(&mut self, other: Inbox) {
                if $(self.$kind.is_empty())&&* {
                    *self = other;
                    return;
                }
                $(merge_queue(&mut self.$kind, other.$kind);)*
            }

            /// The first count of each kind that has answers.
            fn heads(&self) -> [Option<(Kind, u64)>; [$(Kind::$kind),*].len()] {
                [$(self.$kind.front().map(|(at, _)| (Kind::$kind, *at))),*]
            }
        }
    };
}

answer_kinds! {
    fills: (Fill, FillExec) = orders.fills;
    untracked_executions: (crate::api::types::Contract, crate::api::types::Execution, FillExec) = orders.untracked_executions;
    forgotten_orders: OrderId = orders.forgotten_orders;
    commission_reports: crate::api::types::CommissionAndFeesReport = orders.commission_reports;
    order_errors: (i64, i64, String) = orders.order_errors;
    order_reports: OrderReport = orders.order_reports;
    what_if_responses: WhatIfResponse = orders.what_if_responses;
    historical_ticks: (ReqId, HistoricalTickData, String, bool) = reference.historical_ticks;
    tbt_errors: (ReqId, i32, String) = market.tbt_errors;
    tbt_trades: TbtTrade = market.tbt_trades;
    tbt_quotes: TbtQuote = market.tbt_quotes;
    tbt_mid_points: TbtMidPoint = market.tbt_mid_points;
    depth_updates: DepthUpdate = market.depth_updates;
    tick_news: TickNews = market.tick_news;
    historical_errors: (ReqId, i32, String) = reference.historical_errors;
    historical_data: (ReqId, HistoricalResponse) = reference.historical_data;
    historical_updates: (ReqId, HistoricalBar) = reference.historical_updates;
    head_timestamps: (ReqId, HeadTimestampResponse) = reference.head_timestamps;
    contract_details: (ReqId, ContractDefinition) = reference.contract_details;
    contract_details_end: ReqId = reference.contract_details_end;
    matching_symbols: (ReqId, Vec<SymbolMatch>) = reference.matching_symbols;
    option_chains: (ReqId, Vec<crate::control::optparams::OptionChain>) = reference.option_chains;
    scanner_data: (ReqId, ScannerResult) = reference.scanner_data;
    historical_news: (ReqId, Vec<NewsHeadline>, bool) = reference.historical_news;
    news_articles: (ReqId, i32, String) = reference.news_articles;
    option_computations: crate::control::optcalc::OptionComputation = reference.option_computations;
    fundamental_data: (ReqId, String) = reference.fundamental_data;
    histogram_data: (ReqId, Vec<HistogramEntry>) = reference.histogram_data;
    real_time_bars: (ReqId, RealTimeBar) = market.real_time_bars;
    historical_schedules: (ReqId, HistoricalScheduleResponse) = reference.historical_schedules;
    connection_notices: (i64, String) = connection_notices;
}

impl Inbox {
    /// Take the answers written so far; returns the count below which
    /// every answer is in the inbox.
    pub(crate) fn take(&mut self, shared: &SharedState) -> u64 {
        // Read first: an answer counted before it is in its queue by then.
        let written = shared.answers_written();
        self.take_queues(shared);
        written
    }

    /// The next run to give: its kind and the count it stops before (the
    /// first answer of another kind, or `written`). None when no answer
    /// below `written` waits.
    #[cfg(test)]
    pub(crate) fn next_run(&self, written: u64) -> Option<(Kind, u64)> {
        match self.next(written, None)? {
            Run::Queue(kind, until) => Some((kind, until)),
            Run::Client(_) => None,
        }
    }

    /// [`next_run`](Self::next_run) with the client's own answers, whose
    /// first count is `client`.
    pub(crate) fn next(&self, written: u64, client: Option<u64>) -> Option<Run> {
        let heads = self.heads();
        let queue = heads.iter().flatten().copied().min_by_key(|(_, at)| *at);
        let first = match (queue, client) {
            (Some((_, q)), Some(c)) => q.min(c),
            (Some((_, q)), None) => q,
            (None, Some(c)) => c,
            (None, None) => return None,
        };
        if first >= written {
            return None;
        }
        match queue {
            Some((kind, at)) if at == first => {
                let other = heads.iter().flatten().filter(|(k, _)| *k != kind).map(|(_, at)| *at)
                    .chain(client).min().unwrap_or(u64::MAX);
                Some(Run::Queue(kind, other.min(written)))
            }
            _ => Some(Run::Client(queue.map_or(u64::MAX, |(_, at)| at).min(written))),
        }
    }
}

/// What to give next: a run of a queue, or of the client's own answers,
/// up to the count it stops before.
pub(crate) enum Run {
    Queue(Kind, u64),
    Client(u64),
}

fn merge_queue<T>(into: &mut VecDeque<(u64, T)>, from: VecDeque<(u64, T)>) {
    if from.is_empty() {
        return;
    }
    into.extend(from);
    into.make_contiguous().sort_by_key(|(at, _)| *at);
}

/// The answers of a queue counted below `until`, in their order.
pub(crate) fn run<T>(queue: &mut VecDeque<(u64, T)>, until: u64) -> impl Iterator<Item = T> + '_ {
    let n = queue.iter().take_while(|(at, _)| *at < until).count();
    queue.drain(..n).map(|(_, item)| item)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_follow_the_counts_across_the_queues() {
        let shared = SharedState::new();
        shared.reference.push_contract_details_end(1);
        shared.orders.push_order_error(2, 200, "a".into());
        shared.orders.push_order_error(3, 200, "b".into());
        shared.reference.push_contract_details_end(4);
        let mut inbox = Inbox::default();
        let written = inbox.take(&shared);
        let mut got = Vec::new();
        while let Some((kind, until)) = inbox.next_run(written) {
            match kind {
                Kind::contract_details_end => got.extend(run(&mut inbox.contract_details_end, until).into_iter().map(|r| format!("end {r}"))),
                Kind::order_errors => got.extend(run(&mut inbox.order_errors, until).into_iter().map(|(id, ..)| format!("error {id}"))),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(got, ["end 1", "error 2", "error 3", "end 4"]);
    }

    #[test]
    fn an_answer_counted_after_the_reading_waits_for_the_next_pass() {
        let shared = SharedState::new();
        shared.reference.push_contract_details_end(1);
        let mut inbox = Inbox::default();
        let written = inbox.take(&shared);
        // Written after the clock was read, before the queues were taken.
        shared.orders.push_order_error(2, 200, "a".into());
        inbox.take_queues(&shared);
        let (kind, until) = inbox.next_run(written).unwrap();
        assert_eq!(kind, Kind::contract_details_end);
        assert_eq!(run(&mut inbox.contract_details_end, until).collect::<Vec<_>>(), [1]);
        assert!(inbox.next_run(written).is_none());
        let written = inbox.take(&shared);
        assert_eq!(inbox.next_run(written).map(|(k, _)| k), Some(Kind::order_errors));
    }
}

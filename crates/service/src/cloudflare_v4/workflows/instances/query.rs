//! Strict instance query and lifecycle request parsing.

use super::*;
use crate::cloudflare_v4::storage::strict_query;
use open_compute_core::workflow::{WorkflowRestartSelector, WorkflowRestartStepType};
use open_compute_storage::scheduler::WorkflowInstanceAction;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Direction {
    Asc,
    Desc,
}

pub(super) struct ListQuery {
    pub(super) page: usize,
    pub(super) per_page: usize,
    pub(super) cursor: Option<String>,
    direction: Direction,
    status: Option<String>,
    date_start: Option<i64>,
    date_end: Option<i64>,
}

impl ListQuery {
    pub(super) fn parse(request: &Request) -> Result<Self, V4Error> {
        let values = strict_query(request)?;
        if values.keys().any(|key| {
            !matches!(
                key.as_str(),
                "page" | "per_page" | "cursor" | "direction" | "status" | "date_start" | "date_end"
            )
        }) {
            return Err(V4Error::InvalidRequest);
        }
        let page = parse_usize(&values, "page", 1, 1, usize::MAX)?;
        let per_page = parse_usize(&values, "per_page", 50, 1, 100)?;
        if values.contains_key("cursor") && values.contains_key("page") {
            return Err(V4Error::InvalidRequest);
        }
        let direction = match values.get("direction").map_or("desc", String::as_str) {
            "asc" => Direction::Asc,
            "desc" => Direction::Desc,
            _ => return Err(V4Error::InvalidRequest),
        };
        let status = values.get("status").cloned();
        if status.as_deref().is_some_and(|status| {
            !matches!(
                status,
                "queued"
                    | "running"
                    | "paused"
                    | "errored"
                    | "terminated"
                    | "complete"
                    | "waitingForPause"
                    | "waiting"
                    | "rollingBack"
            )
        }) {
            return Err(V4Error::InvalidRequest);
        }
        let date_start = timestamp(values.get("date_start"))?;
        let date_end = timestamp(values.get("date_end"))?;
        if date_start
            .zip(date_end)
            .is_some_and(|(start, end)| start > end)
        {
            return Err(V4Error::InvalidRequest);
        }
        Ok(Self {
            page,
            per_page,
            cursor: values.get("cursor").cloned(),
            direction,
            status,
            date_start,
            date_end,
        })
    }

    pub(super) fn binding(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.per_page,
            if self.direction == Direction::Asc {
                "asc"
            } else {
                "desc"
            },
            self.status.as_deref().unwrap_or(""),
            self.date_start
                .map_or_else(String::new, |value| value.to_string()),
            self.date_end
                .map_or_else(String::new, |value| value.to_string())
        )
    }

    pub(super) fn matches(&self, instance: &WorkflowInstanceInspection) -> bool {
        self.status.as_deref().is_none_or(|status| {
            status_name(
                instance.status,
                instance.durable.rollback_requested,
                instance.durable.pause_requested,
            ) == status
        }) && self
            .date_start
            .is_none_or(|start| instance.created_at_ms >= start)
            && self
                .date_end
                .is_none_or(|end| instance.created_at_ms <= end)
    }

    pub(super) fn after(&self, instance: &WorkflowInstanceInspection, cursor: &Position) -> bool {
        let candidate = (instance.created_at_ms, instance.id.to_string());
        let position = (cursor.created_at_ms, cursor.instance_id.to_string());
        if self.direction == Direction::Asc {
            candidate > position
        } else {
            candidate < position
        }
    }

    pub(super) const fn descending(&self) -> bool {
        matches!(self.direction, Direction::Desc)
    }
}

#[derive(Clone, Copy)]
pub(super) struct DetailQuery {
    pub(super) simple: bool,
    pub(super) order: Direction,
}

impl DetailQuery {
    pub(super) fn parse(request: &Request) -> Result<Self, V4Error> {
        let values = strict_query(request)?;
        if values
            .keys()
            .any(|key| !matches!(key.as_str(), "simple" | "order"))
        {
            return Err(V4Error::InvalidRequest);
        }
        let simple = match values.get("simple").map_or("false", String::as_str) {
            "true" => true,
            "false" => false,
            _ => return Err(V4Error::InvalidRequest),
        };
        let order = match values.get("order").map_or("asc", String::as_str) {
            "asc" => Direction::Asc,
            "desc" => Direction::Desc,
            _ => return Err(V4Error::InvalidRequest),
        };
        Ok(Self { simple, order })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StatusBody {
    status: String,
    rollback: Option<bool>,
    from: Option<RestartFrom>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestartFrom {
    name: String,
    count: Option<u32>,
    #[serde(rename = "type")]
    step_type: Option<WorkflowRestartStepType>,
}

pub(super) enum StatusAction {
    Modify(WorkflowInstanceAction),
    Rollback,
    Restart(Option<WorkflowRestartSelector>),
}

impl StatusBody {
    pub(super) fn validate(self) -> Result<StatusAction, V4Error> {
        match self.status.as_str() {
            "pause" if self.rollback.is_none() && self.from.is_none() => {
                Ok(StatusAction::Modify(WorkflowInstanceAction::Pause))
            }
            "resume" if self.rollback.is_none() && self.from.is_none() => {
                Ok(StatusAction::Modify(WorkflowInstanceAction::Resume))
            }
            "terminate" if self.from.is_none() => {
                if self.rollback.unwrap_or(false) {
                    Ok(StatusAction::Rollback)
                } else {
                    Ok(StatusAction::Modify(WorkflowInstanceAction::Terminate))
                }
            }
            "restart" if self.rollback.is_none() => Ok(StatusAction::Restart(
                self.from
                    .map(|from| WorkflowRestartSelector {
                        name: from.name,
                        count: from.count.unwrap_or(1),
                        step_type: from.step_type,
                    })
                    .map(|selector| {
                        selector
                            .validate()
                            .map(|()| selector)
                            .map_err(|_| V4Error::InvalidField("/from"))
                    })
                    .transpose()?,
            )),
            _ => Err(V4Error::InvalidRequest),
        }
    }
}

fn parse_usize(
    values: &BTreeMap<String, String>,
    key: &str,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> Result<usize, V4Error> {
    let value = values.get(key).map_or(Ok(default), |value| {
        value.parse().map_err(|_| V4Error::InvalidRequest)
    })?;
    (minimum..=maximum)
        .contains(&value)
        .then_some(value)
        .ok_or(V4Error::InvalidRequest)
}

fn timestamp(value: Option<&String>) -> Result<Option<i64>, V4Error> {
    value
        .map(|value| {
            value
                .parse::<jiff::Timestamp>()
                .map(jiff::Timestamp::as_millisecond)
                .map_err(|_| V4Error::InvalidRequest)
        })
        .transpose()
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;

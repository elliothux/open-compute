//! Daemon-wide admission for the actual Prometheus series exposed by instances.

use open_compute_core::{ErrorCode, InstanceId, PlatformError};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

#[derive(Debug)]
pub(crate) struct MetricSeriesBudget {
    max: u64,
    by_instance: Mutex<HashMap<InstanceId, HashSet<String>>>,
}

impl MetricSeriesBudget {
    pub(crate) fn new(max: u64) -> Self {
        Self {
            max,
            by_instance: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn render(&self, id: InstanceId, body: &str) -> Result<String, PlatformError> {
        let mut output = String::with_capacity(body.len());
        let mut current = HashSet::new();
        for line in body.lines() {
            if line.is_empty() || line.starts_with('#') {
                output.push_str(line);
                output.push('\n');
                continue;
            }
            let (name, value, labelled) = if let Some(end) = line.rfind("} ") {
                (&line[..end], &line[end + 1..], true)
            } else if let Some(end) = line.find(' ') {
                (&line[..end], &line[end..], false)
            } else {
                return Err(invalid_series());
            };
            let start = output.len();
            if labelled {
                output.push_str(name);
                output.push_str(",instance_id=\"");
            } else {
                output.push_str(name);
                output.push_str("{instance_id=\"");
            }
            output.push_str(id.as_str());
            output.push_str("\"}");
            current.insert(output[start..].to_owned());
            output.push_str(value);
            output.push('\n');
        }
        let mut by_instance = self.by_instance.lock().map_err(|_| invalid_series())?;
        let used_elsewhere = by_instance
            .iter()
            .filter(|(other, _)| **other != id)
            .fold(0_u64, |count, (_, series)| {
                count.saturating_add(series.len() as u64)
            });
        if used_elsewhere.saturating_add(current.len() as u64) > self.max {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "daemon metric series capacity is exhausted",
            ));
        }
        by_instance.insert(id, current);
        Ok(output)
    }

    pub(crate) fn remove(&self, id: &InstanceId) {
        if let Ok(mut by_instance) = self.by_instance.lock() {
            by_instance.remove(id);
        }
    }
}

fn invalid_series() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "metric series registry is unavailable",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_label_with_closing_brace_and_space_stays_intact() {
        let id = InstanceId::generate();
        let budget = MetricSeriesBudget::new(1);
        let body = "# TYPE example gauge\nexample{value=\"a} b\"} 1\n";
        assert_eq!(
            budget.render(id, body).unwrap(),
            format!("# TYPE example gauge\nexample{{value=\"a}} b\",instance_id=\"{id}\"}} 1\n")
        );
    }
}

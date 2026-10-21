//! Application identity and conservative process-tree grouping policy.

use crate::{CaptureError, Result};

pub(crate) const SOURCE_PREFIX: &str = "process:";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ApplicationId {
    pub(crate) process_id: u32,
    pub(crate) created: u64,
}

impl ApplicationId {
    pub(crate) fn parse(source: &str) -> Result<Self> {
        let invalid = || CaptureError::InvalidConfig("invalid application source ID");
        let rest = source.strip_prefix(SOURCE_PREFIX).ok_or_else(invalid)?;
        let (process, created) = rest.split_once(':').ok_or_else(invalid)?;
        if process.is_empty()
            || created.is_empty()
            || !process.bytes().all(|value| value.is_ascii_digit())
            || !created.bytes().all(|value| value.is_ascii_digit())
        {
            return Err(invalid());
        }
        let id = Self {
            process_id: process.parse().map_err(|_| invalid())?,
            created: created.parse().map_err(|_| invalid())?,
        };
        if id.process_id == 0 || id.created == 0 {
            return Err(invalid());
        }
        Ok(id)
    }

    #[cfg(any(windows, test))]
    pub(crate) fn source_id(self) -> String {
        format!("{SOURCE_PREFIX}{}:{}", self.process_id, self.created)
    }
}

/// Never cross an executable boundary to a browser launcher, shell or host.
/// A newer parent object is a reused PID, not the parent in the snapshot.
#[cfg(any(windows, test))]
pub(crate) fn same_application_parent(
    child: ApplicationId,
    child_path: &str,
    parent: ApplicationId,
    parent_path: &str,
) -> bool {
    child.process_id != parent.process_id
        && parent.created <= child.created
        && !child_path.is_empty()
        && child_path.eq_ignore_ascii_case(parent_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_id_is_bound_to_pid_and_creation_time() {
        let id = ApplicationId {
            process_id: 42,
            created: 133_000_000_000_000_000,
        };
        assert_eq!(ApplicationId::parse(&id.source_id()).unwrap(), id);
        let restarted = ApplicationId {
            created: id.created + 1,
            ..id
        };
        assert_ne!(id.source_id(), restarted.source_id());
        for source in [
            "process:0:1",
            "process:4:0",
            "process:4",
            "process:4:5:6",
            "process:+4:5",
            "process:4:-1",
            "process:4294967296:1",
            "default",
        ] {
            assert!(ApplicationId::parse(source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn grouping_does_not_escape_app_or_accept_reused_parent_pid() {
        let child = ApplicationId {
            process_id: 100,
            created: 20,
        };
        let parent = ApplicationId {
            process_id: 80,
            created: 10,
        };
        assert!(same_application_parent(
            child,
            r"C:\Browser\browser.exe",
            parent,
            r"c:\browser\BROWSER.exe"
        ));
        assert!(!same_application_parent(
            child,
            r"C:\Browser\browser.exe",
            parent,
            r"C:\Windows\explorer.exe"
        ));
        assert!(!same_application_parent(
            child,
            "browser.exe",
            ApplicationId {
                created: 21,
                ..parent
            },
            "browser.exe"
        ));
        assert!(!same_application_parent(
            child,
            "browser.exe",
            child,
            "browser.exe"
        ));
        assert!(!same_application_parent(child, "", parent, ""));
    }
}

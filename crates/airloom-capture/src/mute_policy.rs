//! Pure ownership policy, separated from endpoint-volume COM calls.

#[derive(Debug, Default)]
pub(crate) struct MuteOwnership {
    original: Option<bool>,
}

impl MuteOwnership {
    pub(crate) fn is_active(&self) -> bool {
        self.original.is_some()
    }

    pub(crate) fn acquire(&mut self, original: bool) {
        self.original = Some(original);
    }

    pub(crate) fn release(
        &mut self,
        changed_externally: bool,
        current_muted: bool,
    ) -> Option<bool> {
        let original = self.original.take()?;
        if changed_externally || !current_muted || original {
            // Never undo another controller's choice, or unmute an endpoint
            // that was already muted before this session acquired it.
            None
        } else {
            Some(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_only_a_change_this_session_still_owns() {
        let mut ownership = MuteOwnership::default();
        assert!(!ownership.is_active());
        ownership.acquire(false);
        assert!(ownership.is_active());
        assert_eq!(ownership.release(false, true), Some(false));
        assert!(!ownership.is_active());
        assert_eq!(ownership.release(false, true), None);
        ownership.acquire(true);
        assert_eq!(ownership.release(false, true), None);
        ownership.acquire(false);
        assert_eq!(ownership.release(true, true), None);
        ownership.acquire(false);
        assert_eq!(ownership.release(false, false), None);
    }
}

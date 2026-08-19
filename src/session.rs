use crate::lsn::Lsn;

#[derive(Default, Debug)]
pub struct SessionState {
    last_write_lsn: Option<Lsn>,
    in_transaction: bool,
}

impl SessionState {
    pub fn record_write(&mut self, lsn: Lsn) {
        if self.last_write_lsn.is_none_or(|current| lsn > current) {
            self.last_write_lsn = Some(lsn);
        }
    }
    pub fn last_write_lsn(&self) -> Option<Lsn> {
        self.last_write_lsn
    }
    pub fn enter_transaction(&mut self) {
        self.in_transaction = true;
    }
    pub fn exit_transaction(&mut self) {
        self.in_transaction = false;
    }
    pub fn in_transaction(&self) -> bool {
        self.in_transaction
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_session() {
        let session = SessionState::default();
        assert!(session.last_write_lsn().is_none());
        assert!(!session.in_transaction());
    }

    #[test]
    fn none_prior_lsn() {
        let mut session = SessionState::default();
        let lsn = Lsn::parse("0/100").unwrap();
        session.record_write(lsn);
        assert_eq!(session.last_write_lsn().unwrap(), lsn);
    }

    #[test]
    fn large_updates() {
        let mut session = SessionState::default();
        session.record_write(Lsn::parse("0/100").unwrap());
        let lsn = Lsn::parse("100/200").unwrap();
        session.record_write(lsn);
        assert_eq!(session.last_write_lsn().unwrap(), lsn);
    }

    #[test]
    fn small_updates() {
        let mut session = SessionState::default();
        let lsn = Lsn::parse("100/200").unwrap();
        session.record_write(lsn);
        session.record_write(Lsn::parse("100/100").unwrap());
        assert_eq!(session.last_write_lsn().unwrap(), lsn);
    }
}

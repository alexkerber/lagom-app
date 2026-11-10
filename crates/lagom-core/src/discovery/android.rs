use hickory_resolver::proto::{op::{Message, ResponseCode}, rr::RData};
use std::ffi::{CStr, CString};

#[cfg(target_os = "android")]
pub(super) async fn mx(domain: String) -> Option<Vec<String>> {
    use std::os::raw::{c_char, c_int};
    extern "C" {
        fn res_query(name: *const c_char, class: c_int, kind: c_int, answer: *mut u8, len: c_int) -> c_int;
    }
    query(domain, |name, answer| {
        // Bionic uses Android's network resolver, including VPN/private DNS, without resolv.conf.
        unsafe { res_query(name.as_ptr(), 1, 15, answer.as_mut_ptr(), answer.len() as c_int) }
    }).await
}

async fn query(
    domain: String,
    resolve: impl FnOnce(&CStr, &mut [u8]) -> i32 + Send + 'static,
) -> Option<Vec<String>> {
    // shortcut: a timed-out blocking query finishes at the OS deadline, use cancellable NDK queries if discovery becomes frequent.
    tokio::task::spawn_blocking(move || {
        let name = CString::new(domain).ok()?;
        let mut answer = vec![0; 65535];
        let len = usize::try_from(resolve(&name, &mut answer)).ok()?;
        let message = Message::from_vec(answer.get(..len)?).ok()?;
        if message.metadata.truncation || message.metadata.response_code != ResponseCode::NoError {
            return None;
        }
        Some(message.answers.iter().filter_map(|record| match &record.data {
            RData::MX(mx) => Some(mx.exchange.to_utf8()),
            _ => None,
        }).collect())
    }).await.ok()?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{discover_with, Discovery};
    use crate::oauth::Provider;
    use hickory_resolver::proto::{op::OpCode, rr::{Name, Record, rdata::MX}};

    fn answer(host: &str) -> Vec<u8> {
        Message::response(0, OpCode::Query).add_answer(Record::from_rdata(
            Name::from_ascii("custom.example").unwrap(), 60,
            RData::MX(MX::new(10, Name::from_ascii(host).unwrap())),
        )).to_vec().unwrap()
    }

    #[tokio::test]
    async fn system_mx_discovers_custom_domains_off_the_async_thread_without_ispdb() {
        for host in ["in1-smtp.messagingengine.com.", "aspmx.l.google.com."] {
            let caller = std::thread::current().id();
            let response = answer(host);
            let result = discover_with("user@custom.example", |domain| query(domain, move |name, buffer| {
                assert_ne!(caller, std::thread::current().id());
                assert_eq!(name.to_str().unwrap(), "custom.example");
                buffer[..response.len()].copy_from_slice(&response);
                response.len() as i32
            }), |_| async { panic!("recognized system MX must not use ISPDB") }).await;
            if host.contains("google") {
                assert_eq!(result, Discovery::OAuth(Provider::Google));
            } else {
                let Discovery::Password { preset, .. } = result else { panic!("expected Fastmail") };
                assert_eq!(preset.imap.host, "imap.fastmail.com");
                assert_eq!(preset.imap.username, "user@custom.example");
            }
        }
    }

    #[tokio::test]
    async fn failed_or_malformed_system_dns_falls_through() {
        for len in [-1, 65536, 0, 12] {
            let result = discover_with("user@custom.example", |domain| query(domain, move |_, _| len),
                |_| async { None }).await;
            assert_eq!(result, Discovery::Manual);
        }
        for response in [
            Message::response(0, OpCode::Query).truncate().to_vec().unwrap(),
            Message::error_msg(0, OpCode::Query, ResponseCode::NXDomain).to_vec().unwrap(),
        ] {
            assert!(query("custom.example".into(), move |_, buffer| {
                buffer[..response.len()].copy_from_slice(&response);
                response.len() as i32
            }).await.is_none());
        }
        assert!(query("bad\0domain".into(), |_, _| panic!("invalid domain queried")).await.is_none());
    }
}

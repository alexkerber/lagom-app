use super::*;
use std::net::IpAddr;

pub(super) fn run(forced: &[String]) -> Result<i32> {
    let identity = match forced {
        [remote] if remote == "remote" => None,
        [remote, flag, id] if remote == "remote" && flag == "--agent" => {
            Some(("agent".into(), id.clone()))
        }
        _ => return Err(INVALID),
    };
    let connection = env::var("SSH_CONNECTION").map_err(|_| INVALID)?;
    let fields: Vec<_> = connection.split_ascii_whitespace().collect();
    let [ip, port, host, host_port] = fields.as_slice() else {
        return Err(INVALID);
    };
    let ip: IpAddr = ip.parse().map_err(|_| INVALID)?;
    host.parse::<IpAddr>().map_err(|_| INVALID)?;
    port.parse::<u16>().map_err(|_| INVALID)?;
    host_port.parse::<u16>().map_err(|_| INVALID)?;

    let original = env::var("SSH_ORIGINAL_COMMAND").map_err(|_| INVALID)?;
    if !original
        .chars()
        .all(|c| c.is_alphanumeric() || " -_.,:/@=+".contains(c))
    {
        return Err(INVALID);
    }
    let mut words = original.split_ascii_whitespace();
    if words.next() != Some("lagom") {
        return Err(INVALID);
    }
    let words: Vec<_> = words.map(str::to_owned).collect();
    let command = words.first().ok_or(INVALID)?;
    if command != "mcp"
        && !TOOLS
            .iter()
            .any(|t| t.cmd.split('.').next() == Some(command))
    {
        return Err(INVALID);
    }
    for flag in words.iter().filter_map(|w| w.strip_prefix("--")) {
        let key = flag.split('=').next().unwrap();
        if key != "json" && key != "account" && !TOOLS.iter().any(|t| t.flags.contains(&key)) {
            return Err(INVALID);
        }
    }
    let mut opts = Options::parse(words)?;
    opts.identity = identity;
    super::run(opts, Some(ip.to_string()))
}

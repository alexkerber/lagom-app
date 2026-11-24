#![cfg(unix)]

use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    process::{Command, Stdio},
    thread,
};

#[test]
fn cli_socket_and_mcp_round_trip() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let dir = temp.path();
    let id = "3f9a1c0b77de";
    fs::create_dir(dir.join("agents")).unwrap();
    fs::write(dir.join("agents").join(format!("{id}.token")), "file\n").unwrap();
    let run = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_lagom"));
        cmd.env("LAGOM_DATA_DIR", dir).env("LAGOM_TOKEN", "environment").args(args);
        cmd
    };
    let success =
        "{ \"v\":1,\"ok\":true,\"result\":{\"messages\":[{\"subject\":\"Hi\"}]},\"extra\":null }\n";
    let denied =
        "{\"v\":1,\"ok\":false,\"error\":{\"code\":\"denied\",\"message\":\"Not approved.\"}}\n";
    let exchanges = [
        ("environment","emails",json!({"filter":"is:unread","page":1,"limit":20}),success),
        ("flag","draft.send",json!({"id":"d1.x"}),denied),
        ("file","thread",json!({"id":"t1.x"}),denied),
        ("environment","accounts",json!({}),"{\"v\":1,\"ok\":true,\"result\":{\"accounts\":[{\"mail\":\"read\",\"calendar\":\"none\"}]}}\n"),
        ("environment","emails",json!({"limit":1}),success),
        ("environment","emails",json!({}),success),
    ];
    let listener = UnixListener::bind(dir.join("agent.sock")).unwrap();
    let server = thread::spawn(move || {
        for (token, cmd, args, response) in exchanges {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            assert_eq!(
                line,
                format!("{{\"v\":1,\"token\":\"{token}\",\"cmd\":\"{cmd}\",\"args\":{args}}}\n")
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    let out = run(&["emails", "--filter", "is:unread", "--page=1", "--limit", "20", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout, success.as_bytes());

    let out = run(&["draft", "send", "d1.x", "--token=flag", "--agent", id]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(out.stderr, b"Not approved.\n");
    let out =
        run(&["--agent", id, "thread", "t1.x", "--token", "flag", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(out.stdout, denied.as_bytes());
    assert!(out.stderr.is_empty());

    let mut mcp = run(&["mcp"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut input = mcp.stdin.take().unwrap();
    for req in [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_mail","arguments":{"limit":1}}}),
    ] {
        writeln!(input, "{req}").unwrap();
    }
    drop(input);
    let out = mcp.wait_with_output().unwrap();
    assert!(out.status.success());
    let replies: Vec<Value> = out
        .stdout
        .as_slice()
        .lines()
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
        .collect();
    assert_eq!(replies.len(), 3);
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|t| t["name"] == "list_mail"));
    assert!(!tools.iter().any(|t| t["name"] == "mail_action" || t["name"] == "list_events"));
    assert_eq!(replies[2]["result"]["isError"], false);
    assert_eq!(replies[2]["result"]["content"][0]["text"], r#"{"messages":[{"subject":"Hi"}]}"#);
    let out = run(&["emails"]).output().unwrap();
    assert_eq!(out.stdout, b"subject\nHi\n");
    server.join().unwrap();

    for (args, status, message) in [
        (
            vec!["accounts", "--agent", "deadbeefcafe"],
            1,
            "Token missing, invalid, expired or revoked.",
        ),
        (
            vec!["accounts", "--agent", "../../escape"],
            1,
            "Token missing, invalid, expired or revoked.",
        ),
        (vec!["accounts"], 1, "lagom is not running, or agent access is off."),
        (vec!["emails", "--limit=101"], 2, "Invalid request."),
    ] {
        let out = run(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(status));
        assert_eq!(String::from_utf8(out.stderr).unwrap().trim_end(), message);
    }
    let skill = dir.join("skill");
    assert!(run(&["skill", "install", "--dir", skill.to_str().unwrap()])
        .status()
        .unwrap()
        .success());
    assert_eq!(
        fs::read(skill.join("SKILL.md")).unwrap(),
        include_bytes!("../assets/skill/SKILL.md")
    );
    assert!(!dir.join("lagom.sqlite3").exists());
}

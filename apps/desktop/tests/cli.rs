#![cfg(unix)]

use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
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
    fs::set_permissions(
        dir.join("agents").join(format!("{id}.token")),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
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
    let out =
        run(&["--json", "emails", "--filter", "is:unread", "--page=1", "--limit", "20", "--json"])
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
    let mut replies = Vec::<Value>::new();
    let mut output = BufReader::new(mcp.stdout.take().unwrap());
    for req in [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_mail","arguments":{"limit":1}}}),
    ] {
        writeln!(input, "{req}").unwrap();
        if req.get("id").is_some() {
            let mut reply = String::new();
            output.read_line(&mut reply).unwrap();
            replies.push(serde_json::from_str(&reply).unwrap());
        }
    }
    for id in [Value::Null, json!(1.25)] {
        writeln!(input, "{}", json!({"jsonrpc":"2.0","id":id,"method":"ping"})).unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["error"]["code"], -32600);
    }
    drop(input);
    let out = mcp.wait_with_output().unwrap();
    assert!(out.status.success());
    assert_eq!(replies.len(), 3);
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().all(|t| t["description"].as_str().is_some_and(|s| !s.is_empty())));
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

#[test]
fn cli_rejects_unsafe_credentials_paths_and_missing_option_values() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let id = "abcdef012345";
    let agents = dir.path().join("agents");
    fs::create_dir(&agents).unwrap();
    let file = agents.join(format!("{id}.token"));
    let run = |data: &std::path::Path, args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_lagom"))
            .env("LAGOM_DATA_DIR", data)
            .env("LAGOM_TOKEN", "fixture")
            .arg("--json")
            .args(args)
            .output()
            .unwrap();
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["error"]["code"].clone()
    };
    for data in ["", "relative"] {
        assert_eq!(run(data.as_ref(), &["--account=x", "accounts"]), "invalid_request");
    }
    assert_eq!(run(dir.path(), &["draft", "create", "--body", "--token=X"]), "invalid_request");
    assert_eq!(run(dir.path(), &["--account", "x", "accounts"]), "app_unreachable");
    fs::write(&file, b"fixture").unwrap();
    for (size, mode) in [(7, 0o644), (257, 0o600)] {
        fs::write(&file, vec![b'x'; size]).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(run(dir.path(), &["--agent", id, "accounts"]), "unauthorized");
    }
    fs::rename(&file, dir.path().join("secret")).unwrap();
    fs::write(dir.path().join("secret"), b"fixture").unwrap();
    std::os::unix::fs::symlink(dir.path().join("secret"), &file).unwrap();
    assert_eq!(run(dir.path(), &["--agent", id, "accounts"]), "unauthorized");
    fs::remove_file(&file).unwrap();
    let upper = id.to_ascii_uppercase();
    fs::hard_link(dir.path().join("secret"), agents.join(format!("{upper}.token"))).unwrap();
    assert_eq!(run(dir.path(), &["--agent", &upper, "accounts"]), "unauthorized");
    let dest = dir.path().join("SKILL.md");
    std::os::unix::fs::symlink(dir.path().join("secret"), &dest).unwrap();
    assert_eq!(
        run(dir.path(), &["skill", "install", "--dir", dir.path().to_str().unwrap()]),
        "unavailable"
    );
    assert_eq!(fs::read(&dest).unwrap(), b"fixture");
}

#[test]
fn cancelling_or_closing_mcp_releases_a_waiting_socket() {
    use std::{io::Read, time::Duration};
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let listener = UnixListener::bind(dir.path().join("agent.sock")).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_lagom"))
        .env("LAGOM_DATA_DIR", dir.path())
        .env("LAGOM_TOKEN", "fixture")
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for id in [1, 2] {
        let req = json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"send_draft","arguments":{"id":"d1.fixture"}}});
        writeln!(input, "{req}").unwrap();
        let (socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut peer = BufReader::new(socket);
        let mut request = String::new();
        peer.read_line(&mut request).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&request).unwrap()["cmd"], "draft.send");
        if id == 1 {
            writeln!(input,"{}",json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":id}})).unwrap();
        } else {
            drop(input);
            assert_eq!(peer.read(&mut [0]).unwrap(), 0);
            break;
        }
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

#[test]
fn undecodable_write_replies_have_uncertain_outcomes() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let listener = UnixListener::bind(dir.path().join("agent.sock")).unwrap();
    let server = thread::spawn(move || {
        for response in [
            "?",
            "{\"v\":9,\"ok\":true,\"result\":{}}",
            "{\"v\":1,\"ok\":false,\"error\":{\"code\":\"unknown\"}}",
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            BufReader::new(&stream).read_line(&mut String::new()).unwrap();
            writeln!(stream, "{response}").unwrap();
        }
    });
    for _ in 0..3 {
        let out = Command::new(env!("CARGO_BIN_EXE_lagom"))
            .env("LAGOM_DATA_DIR", dir.path())
            .env("LAGOM_TOKEN", "fixture")
            .args(["--json", "draft", "send", "id"])
            .output()
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap()["error"]["code"],
            "uncertain"
        );
    }
    server.join().unwrap();
}

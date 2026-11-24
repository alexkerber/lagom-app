# Connect a remote agent over SSH

The SSH server needs OpenSSH 9.1 or later for the UTC `Z` suffix in `expiry-time`. [OpenSSH 9.1 release notes](https://www.openssh.org/txt/release-9.1)

Check the effective `sshd_config` for the lagom user before installing the key. Keep `AcceptEnv` limited to locale variables, or leave it unset to accept none. Remove broader entries, including those in included configuration files. Use these settings if forwarding locales:

```sshd_config
AcceptEnv LANG LC_*
PermitUserEnvironment no
```

1. On the Mac or Linux computer running lagom, open **Settings → AI Agents**. Enable agent access and choose the global access levels for the accounts you want to share.
2. Click **Add remote agent**. Enter a host label and paste the agent’s SSH public key. Select its accounts and maximum access. Enter the source IP addresses or CIDR ranges that the SSH server sees, separated by commas. Use network addresses with no host bits and prefixes of /16 or longer for IPv4 or /48 or longer for IPv6. Review the normalized ranges. Choose an expiry date within 365 days. Access ends at midnight UTC at the start of that date.
3. Click **Create agent**. Copy the complete SSH line into the lagom user’s `~/.ssh/authorized_keys`. Use a dedicated key with only that entry. The line selects the agent’s private token file by ID. Only choose **Show token** if another setup needs the token itself. Click **Done** when finished.
4. From the remote host, using an SSH alias such as `mac` that selects this key and user, run `ssh mac lagom emails --json`. For MCP, set the command to `ssh` and the arguments to `["mac", "lagom", "mcp"]`. The SSH CLI splits arguments on spaces. Individual arguments cannot contain spaces or quotes. Use `ssh mac lagom mcp` for searches with several words and for drafts. Keep the desktop app running.
5. Click **Revoke** beside the agent to stop further requests. Delete its line from `authorized_keys`. **Remove** clears the revoked agent from Settings.

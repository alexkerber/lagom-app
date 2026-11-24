# Connect a remote agent over SSH

1. On the Mac or Linux computer running lagom, open **Settings → AI Agents**. Enable agent access and choose the global access levels for the accounts you want to share.
2. Click **Add remote agent**. Enter a host label and paste the agent’s SSH public key. Select its accounts and maximum access. Enter the source IP addresses or CIDR ranges that the SSH server sees, separated by commas. Choose an expiry date within 365 days. Access ends at midnight UTC at the start of that date.
3. Click **Create agent**. Copy the complete SSH line into the lagom user’s `~/.ssh/authorized_keys`. Use a dedicated key with only that entry. The line selects the agent’s private token file by ID. The token is shown once for other setups. Click **Done** when finished.
4. From the remote host, using an SSH alias such as `mac` that selects this key and user, run `ssh mac lagom emails --json`. For MCP, set the command to `ssh` and the arguments to `["mac", "lagom", "mcp"]`. Keep the desktop app running.
5. Click **Revoke** beside the agent to stop further requests. Delete its line from `authorized_keys`. **Remove** clears the revoked agent from Settings.

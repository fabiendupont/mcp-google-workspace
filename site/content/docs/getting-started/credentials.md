+++
title = "Credentials"
description = "Set up Google OAuth2 credentials"
date = 2026-06-12T00:00:00+00:00
updated = 2026-06-12T00:00:00+00:00
draft = false
weight = 20
template = "docs/page.html"
[extra]
lead = "Configure authentication for Google APIs."
toc = true
top = false
+++

## Option A: GWS CLI (recommended for local development)

The [GWS CLI](https://github.com/googleworkspace/cli) stores credentials in your OS keyring. The MCP server reads them automatically.

```bash
cargo install google-workspace-cli
gws auth login
```

You can also export credentials to a file:

```bash
gws auth export --unmasked > credentials.json
```

```toml
[server]
credentials_file = "credentials.json"
project_id = "my-project-123456"
```

## Option B: Application Default Credentials

```bash
gcloud auth application-default login \
  --scopes=https://www.googleapis.com/auth/drive,\
https://www.googleapis.com/auth/gmail.modify,\
https://www.googleapis.com/auth/calendar
```

## Option C: Service account (recommended for production)

1. Create a service account in [Google Cloud Console](https://console.cloud.google.com/) > **IAM & Admin** > **Service Accounts**
2. Download the JSON key file
3. Enable the APIs you need under **APIs & Services** > **Enabled APIs**
4. Share Drive folders or calendars with the service account email
5. Reference in policy:

```toml
[server]
credentials_file = "/path/to/service-account-key.json"
```

## Credential priority

| Priority | Source |
|----------|--------|
| 1 | `GOOGLE_WORKSPACE_CLI_TOKEN` env var |
| 2 | `credentials_file` in policy TOML |
| 3 | `GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE` env var |
| 4 | `~/.config/gws/credentials.json` |
| 5 | `gws auth export --unmasked` (OS keyring) |
| 6 | `GOOGLE_APPLICATION_CREDENTIALS` env var |
| 7 | `~/.config/gcloud/application_default_credentials.json` |

## Quota project

When using OAuth2 user credentials, set the project ID for quota attribution:

```toml
[server]
project_id = "my-project-123456"
```

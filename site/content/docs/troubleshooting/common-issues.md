+++
title = "Common issues"
description = "Troubleshooting guide"
date = 2026-06-12T00:00:00+00:00
updated = 2026-07-28T00:00:00+00:00
draft = false
weight = 10
template = "docs/page.html"
[extra]
lead = "Authentication errors, policy errors, helper tool issues, and container issues."
toc = true
top = false
+++

## Authentication errors

**"invalid_client"** -- The client_secret is truncated. Use `gws auth export --unmasked`.

**"No credentials found"** -- Check the [credential chain](../../security/credential-chain/).

**"permission to use project"** -- Set `project_id` in the policy `server` object to match your GCP project.

## Policy errors

**"Service 'X' is not allowed"** -- Add the service to the `services` array. Every denial includes a `Fix:` hint with the exact JSON to add.

**"Value 'X' for 'param' is not allowed"** -- The parameter value is not in the constraint's `values` list. Add it or use a different value.

**"Write denied: 'param' must be specified"** -- When constraints are configured for a body parameter, write operations must include that parameter. For Drive, include `parents` in the request body.

**"Method 'X' is denied by policy"** -- The method is in `denied_methods`. Note that suffix matching applies: `"messages.send"` matches the full API method `"users.messages.send"`. Remove or narrow the denylist entry.

## Constraint errors

**"Constraints are configured for 'X' but it was not specified"** -- A constraint exists on a path/query parameter but the agent didn't provide it. Include the parameter in the request.

**"Write denied via addParents: 'X' is read-only"** -- The folder ID in `addParents` or `removeParents` has `read-only` access. Change it to `read-write` in the policy.

## Helper tool errors

**"Use gws_drive_* helpers instead"** -- The model tried to call the generic `drive` tool, but Drive has dedicated helpers. Services with helpers (Drive, Docs, Sheets, Slides, Gmail) block the generic tool. Use the specific helper (e.g., `gws_drive_list` instead of `drive` with `resource: "files", method: "list"`).

**Helper tool not found** -- In lazy mode (the default), helper tools are not visible until the service is discovered. The model must first call `gws_discover(service="drive")` to activate Drive helpers. Use `--eager-tools` to load all tools at startup.

## Gmail errors

**"Label 'X' is not allowed"** -- The policy has `allowed_labels` configured and the message belongs to a label not in the list. Add the label to `allowed_labels` in the Gmail service config.

**"Insufficient scopes for contacts"** -- The `gws_gmail_contacts` tool uses the People API, which requires the `https://www.googleapis.com/auth/contacts.readonly` scope. Re-authorize with this scope included.

**"Cannot send: messages.send is denied"** -- The `denied_methods` list includes `"messages.send"`. Note suffix matching: this also blocks `"users.messages.send"`. Remove it from the denylist to allow sending.

## Audit log issues

**Audit log not writing** -- Check file permissions on the `--audit-log` path. The server must be able to create and append to the file.

**Large audit log** -- The audit log grows unbounded. Set up log rotation externally (e.g., `logrotate`) for long-running deployments.

## Policy reload issues

**"Policy reload failed"** -- The JSON in the policy file is invalid. The server logs the parse error and keeps the current policy. Fix the JSON and send SIGHUP again.

**SIGHUP not working** -- Live reload is only available with `--http` transport and `--policy` flag. Not available in stdio mode.

## Container errors

**"Permission denied"** -- On SELinux systems, use `:Z` flag on volume mounts:

```bash
podman run -v ./policy.json:/etc/mcp-google-workspace/policy.json:ro,Z ...
```

## Upload issues

**Large uploads** -- Files over 10 MB automatically use resumable upload. The server initiates the session and returns a task handle. The agent continues with `upload_handle` + `media_chunk`.

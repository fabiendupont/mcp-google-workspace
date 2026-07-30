---
name: work-with-calendar
description: Full workflow for listing, creating, and managing calendar events
arguments:
  - name: calendar_id
    description: Calendar ID (default primary)
    required: false
---

## Listing events

List upcoming events from the primary calendar:

```json
{
  "name": "gws_calendar_list",
  "arguments": { "max_results": 10 }
}
```

Events include `myStatus` (accepted/declined/tentative) — use it to filter out declined events when summarizing a schedule.

To search for specific events:

```json
{
  "name": "gws_calendar_list",
  "arguments": { "query": "standup", "time_min": "2026-07-28T00:00:00Z" }
}
```

## Getting event details

```json
{
  "name": "gws_calendar_get",
  "arguments": { "event_id": "EVENT_ID" }
}
```

## Creating an event

Timed event:

```json
{
  "name": "gws_calendar_create",
  "arguments": {
    "summary": "Team Sync",
    "start": "2026-07-30T14:00:00+02:00",
    "end": "2026-07-30T14:30:00+02:00",
    "location": "Room A",
    "attendees": "alice@example.com, bob@example.com",
    "calendar_id": "{{calendar_id|primary}}"
  }
}
```

All-day event:

```json
{
  "name": "gws_calendar_create",
  "arguments": {
    "summary": "Company Holiday",
    "start": "2026-12-25",
    "calendar_id": "{{calendar_id|primary}}"
  }
}
```

## Updating an event

Only the fields you provide are changed:

```json
{
  "name": "gws_calendar_update",
  "arguments": {
    "event_id": "EVENT_ID",
    "location": "Room B",
    "description": "Updated: moving to Room B"
  }
}
```

## Finding free time

Check free/busy before scheduling:

```json
{
  "name": "gws_calendar_freebusy",
  "arguments": {
    "time_min": "2026-07-30T08:00:00Z",
    "time_max": "2026-07-30T18:00:00Z",
    "calendar_ids": "primary"
  }
}
```

The response shows busy slots. Find gaps between them for scheduling.

## Deleting an event

```json
{
  "name": "gws_calendar_delete",
  "arguments": { "event_id": "EVENT_ID" }
}
```

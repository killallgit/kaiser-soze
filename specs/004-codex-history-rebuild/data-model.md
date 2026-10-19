# Data model

**History** (per socket, in the Codex parser):
- `last_reply`: id of the last `response.created`, or none
- `system`, `tools`: from the first request, replaced by a later request that sets them
- `messages`: whole conversation so far, including stored reply messages
- `bytes`: serialized size; `over_bound` once past `MAX_REQUEST_BODY`
- `pending`: items of the open reply, committed on `response.completed`

**Rules**: no `previous_response_id` resets History. `previous_response_id == last_reply` emits `messages + new`. Anything else, or `over_bound`, emits the request as sent with `partial: true` and leaves History unchanged.

**Conversation payload** gains `partial: bool` and `warmup: bool`; the record copies both. Existing fields unchanged.

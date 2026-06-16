#!/usr/bin/env python3
"""A starter NurSearch plugin in pure Python (no dependencies).

It speaks the protocol directly over stdio. See docs/AGENTS-PLUGIN-GUIDE.md for
the full message and view reference. Replace the query/activate/event logic with
your own.
"""
import sys
import json

# Messages read from the host while waiting for a host-call result. They are
# replayed by read_message() so the main loop never loses them.
_pending = []
_next_id = 0


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def read_raw():
    """Read one message directly from stdin, or None at EOF."""
    for line in sys.stdin:
        if line.strip():
            return json.loads(line)
    return None


def read_message():
    """Next message for the main loop: buffered ones first, then stdin."""
    if _pending:
        return _pending.pop(0)
    return read_raw()


def host_call(call):
    """Invoke a host capability and block for its result. Any other host message
    that arrives first is buffered for the main loop instead of being dropped."""
    global _next_id
    call_id = _next_id
    _next_id += 1
    send({"type": "hostCall", "id": call_id, "call": call})
    while True:
        msg = read_raw()
        if msg is None:
            return {}
        if msg.get("type") == "hostResult" and msg.get("id") == call_id:
            return msg.get("outcome", {})
        _pending.append(msg)


def on_query(text):
    # Root contributions: flat, rankable items.
    return [{
        "id": "open",
        "title": f"Echo: {text}",
        "subtitle": "Open a detail view",
        "commandId": "open",
        "score": 5000,
    }]


def on_activate(command_id, item_id):
    # Return the first view of the session.
    if command_id == "open":
        return {"type": "render", "view": {
            "type": "detail",
            "title": "Hello",
            "markdown": "This is your plugin's detail view.",
            "actions": [{"id": "copy", "title": "Copy", "kind": {"do": "copy", "text": "Hello"}}],
        }}
    return None


def on_event(event):
    # Handle input/select/action/submit within the session.
    return None


def send_response(response, generation):
    """Emit a render/pop/close reply, stamping it with the triggering generation
    so the host can drop stale messages from older input."""
    if not response:
        return
    if response.get("type") in ("render", "pop", "close"):
        response["generation"] = generation
    send(response)


def main():
    while True:
        msg = read_message()
        if msg is None:
            break
        t = msg.get("type")
        if t == "initialize":
            send({"type": "initialized", "protocolVersion": 1})
        elif t == "query":
            send({"type": "results", "generation": msg["generation"],
                  "done": True, "items": on_query(msg["text"])})
        elif t == "activate":
            send_response(on_activate(msg["commandId"], msg.get("itemId")),
                          msg.get("generation", 0))
        elif t == "event":
            send_response(on_event(msg["event"]), msg.get("generation", 0))
        elif t == "shutdown":
            break


if __name__ == "__main__":
    main()

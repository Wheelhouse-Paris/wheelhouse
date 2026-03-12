#!/usr/bin/env python3
"""Example 2: Publish and subscribe with core types.

Run with a live Wheelhouse:
    python examples/02_publish_subscribe.py

Run in mock mode (no Wheelhouse needed):
    python examples/02_publish_subscribe.py --mock
    WH_MOCK=1 python examples/02_publish_subscribe.py
"""
import sys, os; sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "sdk", "python"))  # noqa: E702

import asyncio

import wheelhouse
from wheelhouse.types import TextMessage


async def main() -> None:
    # Use mock mode if --mock flag or WH_MOCK env var is set
    use_mock = "--mock" in sys.argv or os.environ.get("WH_MOCK") == "1"

    async with await wheelhouse.connect(mock=use_mock) as conn:
        received: list[TextMessage] = []

        async def on_message(msg: TextMessage) -> None:
            received.append(msg)
            print(f"Received: {msg.content}")

        await conn.subscribe("notifications", on_message)

        msg = TextMessage(content="Hello from Wheelhouse SDK!")
        await conn.publish("notifications", msg)
        print(f"Published: {msg.content}")

        if use_mock:
            # In mock mode, messages are echoed immediately
            assert len(received) == 1, f"Expected 1 message, got {len(received)}"
            print("Mock round-trip verified.")


asyncio.run(main())

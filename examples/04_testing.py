"""Example 4: Testing with Mock Mode — no Wheelhouse or Podman required.

Demonstrates how to write pytest tests for your surface code using
wheelhouse.connect(mock=True). All publish/subscribe interactions work
in-memory without any external dependencies.

Run this example:
    python examples/04_testing.py
"""

import asyncio
import sys
from pathlib import Path

# Allow running from project root
sys.path.insert(0, str(Path(__file__).parent.parent / "sdk" / "python"))

import wheelhouse
from wheelhouse.types import TextMessage


async def demo_mock_testing():
    """Demonstrate mock mode testing patterns."""

    # 1. Connect in mock mode — no Wheelhouse needed
    conn = await wheelhouse.connect(mock=True)

    # 2. Set up a subscriber to capture messages
    received = []

    async def on_message(msg):
        received.append(msg)

    await conn.subscribe("notifications", on_message)

    # 3. Publish a message — auto-echoed to subscribers
    await conn.publish("notifications", TextMessage(content="Test alert"))

    # 4. Assert on received messages
    assert len(received) == 1, f"Expected 1 message, got {len(received)}"
    assert received[0].content == "Test alert"
    print("  Subscriber received message with correct content")

    # 5. Use get_published() to inspect what was sent
    published = conn.get_published("notifications")
    assert len(published) == 1
    print(f"  Published {len(published)} message(s) to 'notifications'")

    # 6. Use get_messages() for TypedMessage inspection
    messages = conn.get_messages()
    assert messages[0].type_name == "TextMessage"
    assert messages[0].is_known is True
    print(f"  Message type: {messages[0].type_name}")

    # 7. Reset state between test scenarios
    conn.reset()
    assert len(conn.published) == 0
    print("  State reset — ready for next test scenario")

    await conn.close()
    print("\nAll mock mode tests passed!")


if __name__ == "__main__":
    print("Wheelhouse SDK — Mock Mode Testing Demo\n")
    asyncio.run(demo_mock_testing())

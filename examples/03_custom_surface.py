#!/usr/bin/env python3
"""Example 3: Custom type + full surface loop.

Demonstrates: @register_type, Surface subclass, publish, subscribe, error handling.

Run in mock mode (no Wheelhouse needed):
    python examples/03_custom_surface.py --mock
    WH_MOCK=1 python examples/03_custom_surface.py
"""
import sys, os; sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "sdk", "python"))  # noqa: E702

import asyncio
from dataclasses import dataclass

import wheelhouse


@wheelhouse.register_type("myapp.Query")
@dataclass
class Query:
    question: str = ""
    user_id: str = ""

    def SerializeToString(self) -> bytes:
        import json
        return json.dumps({"question": self.question, "user_id": self.user_id}).encode()

    @classmethod
    def FromString(cls, data: bytes) -> "Query":
        import json
        obj = json.loads(data)
        return cls(question=obj.get("question", ""), user_id=obj.get("user_id", ""))


class AssistantSurface(wheelhouse.Surface):
    """A custom surface that sends queries and prints responses."""

    async def ask(self, question: str) -> None:
        try:
            await self.publish("assistant", Query(question=question, user_id="demo"))
            print(f"Asked: {question}")
        except wheelhouse.PublishTimeout:
            print("Timed out waiting for acknowledgement.")


async def main() -> None:
    use_mock = "--mock" in sys.argv or os.environ.get("WH_MOCK") == "1"

    async with await wheelhouse.connect(mock=use_mock) as conn:
        surface = AssistantSurface(conn)

        async def on_reply(msg: Query) -> None:
            print(f"Reply: {msg.question}")

        await conn.subscribe("assistant", on_reply)
        await surface.ask("What is Wheelhouse?")

    print("Surface loop complete.")


asyncio.run(main())

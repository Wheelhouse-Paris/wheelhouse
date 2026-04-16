#!/usr/bin/env python3
"""Re-baseline the golden corpus expected outputs.

Run this script after changing the decision policy or prompt to regenerate
expected outputs from the current ``decide()`` logic.  Each sample's
``mock_llm_response`` is fed through ``decide()`` and the ``expected``
section is overwritten with the actual result.

Usage:
    cd sdk/python
    uv run python scripts/rebaseline_golden.py

After running, review the diff and commit with a message documenting the
reason for the re-baseline.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

# Ensure the SDK is importable when run from sdk/python/.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from wheelhouse.skills.librarian import ConversationMessage, LibraryState, decide

GOLDEN_DIR = Path(__file__).resolve().parent.parent / "tests" / "golden" / "librarian"


def rebaseline() -> None:
    changed = 0
    total = 0

    for path in sorted(GOLDEN_DIR.glob("*.json")):
        total += 1
        with open(path) as f:
            data = json.load(f)

        inp = data["input"]
        segment = [
            ConversationMessage(
                role=m["role"],
                content=m["content"],
                timestamp_ms=m.get("timestamp_ms", 0),
            )
            for m in inp["segment"]
        ]
        library_state = LibraryState(
            index_content=inp["library_state"].get("index_content", ""),
            existing_pages=inp["library_state"].get("existing_pages", {}),
        )

        mock_response = inp["mock_llm_response"]

        def mock_llm_fn(system_prompt: str, user_content: str) -> str:
            return mock_response

        result = decide(
            segment=segment,
            library_state=library_state,
            locale=inp["locale"],
            llm_fn=mock_llm_fn,
        )

        new_expected = {
            "reason": result.reason,
            "committed": result.committed,
            "page_path": result.page_path,
            "content": result.content,
            "schema_version": result.schema_version,
        }

        old_expected = data.get("expected", {})
        if new_expected != old_expected:
            changed += 1
            print(f"  CHANGED: {path.name}")
            for key in new_expected:
                if new_expected[key] != old_expected.get(key):
                    print(f"    {key}: {old_expected.get(key)!r} -> {new_expected[key]!r}")

        data["expected"] = new_expected
        with open(path, "w") as f:
            json.dump(data, f, indent=2, ensure_ascii=False)
            f.write("\n")

    print(f"\nRebaseline complete: {changed}/{total} samples changed.")
    if changed:
        print("Review the diff and commit with a message documenting the reason.")


if __name__ == "__main__":
    rebaseline()

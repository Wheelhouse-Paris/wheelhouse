#!/usr/bin/env python3
"""Example 1: Register a custom type — no Wheelhouse needed."""
import sys, os; sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "sdk", "python"))  # noqa: E702

import wheelhouse
from dataclasses import dataclass

@wheelhouse.register_type("myapp.SensorReading")
@dataclass
class SensorReading:
    sensor_id: str = ""
    value: float = 0.0
    unit: str = ""

print(f"Registered type: {SensorReading._wh_type_name}")
print(f"Fields: {list(SensorReading.__dataclass_fields__.keys())}")

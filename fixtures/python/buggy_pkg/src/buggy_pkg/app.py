"""A small module containing deliberate, realistic defects.

The codesafe test-suite uses this file to verify that diagnostics are localized
to the right folder, file, function and line.
"""

import os  # unused import (ruff F401)
import time


def slow_sum(n: int) -> int:
    """Intentionally slow accumulation loop (optimizer: use sum())."""
    total = 0
    for i in range(len(range(n))):
        total += i
    return total


def parse_config(path, defaults={}):  # mutable default argument
    """Parse a tiny key=value config file."""
    values = defaults
    handle = open(path)  # no context manager
    for line in handle.read().splitlines():
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        values[key.strip()] = value.strip()
    return values


def rate_limit(bucket, requests):
    """Nested loop with quadratic behaviour."""
    allowed = 0
    for request in requests:
        for key in bucket.keys():
            if request == key:
                allowed += 1
    return allowed


class TokenBucket:
    """Token bucket with a blocking refill path and a shared default."""

    def __init__(self, rate, capacity, tokens=[]):  # mutable default
        self.rate = rate
        self.capacity = capacity
        self.tokens = tokens

    def refill(self):
        while len(self.tokens) < self.capacity:
            self.tokens.append(1)
            time.sleep(0.001)
        return self.tokens

    def allow(self, cost):
        if len(self.tokens) == 0:
            return False
        self.tokens.pop()
        return True

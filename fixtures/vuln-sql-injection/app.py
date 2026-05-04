"""Tiny vulnerable Flask-shaped fixture.

Used as the planted-positive target for OpenAudit dual-agent acceptance
tests: the auditor should detect that `q` is concatenated into a raw SQL
string (CWE-89), and the reviewer should confirm.

DO NOT USE THIS PATTERN IN REAL CODE.
"""

from __future__ import annotations

import sqlite3
from typing import Any


DB_PATH = "/tmp/fixture.db"


def search(q: str) -> list[Any]:
    """Vulnerable: `q` is interpolated directly into the SQL string."""
    conn = sqlite3.connect(DB_PATH)
    cur = conn.cursor()
    sql = "SELECT id, title FROM articles WHERE title LIKE '%" + q + "%'"
    cur.execute(sql)
    return cur.fetchall()


def lookup(user_id: str) -> Any:
    """Also vulnerable: f-string interpolation into raw SQL."""
    conn = sqlite3.connect(DB_PATH)
    cur = conn.cursor()
    cur.execute(f"SELECT email FROM users WHERE id = {user_id}")
    return cur.fetchone()

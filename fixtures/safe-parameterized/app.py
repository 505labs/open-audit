"""Safe counterpart of vuln-sql-injection/app.py.

Same shape, but every query uses parameterized binding via the DB-API's
`cursor.execute(sql, params)` form. Used as a false-positive bait fixture:
a careless auditor might draft a finding for the visible `WHERE` clause,
but the reviewer should refute it on the basis that the placeholder/params
binding is correct.
"""

from __future__ import annotations

import sqlite3
from typing import Any


DB_PATH = "/tmp/fixture.db"


def search(q: str) -> list[Any]:
    """Safe: `q` is bound as a parameter, never interpolated."""
    conn = sqlite3.connect(DB_PATH)
    cur = conn.cursor()
    cur.execute(
        "SELECT id, title FROM articles WHERE title LIKE ?",
        (f"%{q}%",),
    )
    return cur.fetchall()


def lookup(user_id: str) -> Any:
    """Safe: parameterized binding."""
    conn = sqlite3.connect(DB_PATH)
    cur = conn.cursor()
    cur.execute("SELECT email FROM users WHERE id = ?", (user_id,))
    return cur.fetchone()

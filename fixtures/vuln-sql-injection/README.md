# vuln-sql-injection fixture

A planted-positive fixture for OpenAudit's dual-agent acceptance tests.

`app.py` contains two clear instances of CWE-89 (SQL injection) — string
concatenation and f-string interpolation into raw SQL. The auditor agent is
expected to surface these; the reviewer agent is expected to confirm them.

Do not use as a template for real code.

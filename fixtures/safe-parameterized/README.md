# safe-parameterized fixture

A negative-control fixture for OpenAudit's dual-agent acceptance tests.

`app.py` mirrors the structure of `vuln-sql-injection/app.py`, but every
query uses the DB-API's parameterized-binding form (`cursor.execute(sql,
params)`). The reviewer agent is expected to refute any finding the auditor
drafts here, citing the parameterized binding.

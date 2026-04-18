# SQL Node Multi-Tenancy via PostgreSQL Row-Level Security

## Goal

Enable the `sql_query` node to enforce per-user data isolation using PostgreSQL Row-Level Security (RLS), so that multiple end-users can share the same database and tables without seeing or modifying each other's data. The LLM generates normal SQL; Postgres enforces the boundaries.

## Architecture

Three new optional fields in the `permissions` config object:

- **`tenant_user_id`** (string, optional): The current user's ID, resolved at runtime via `${ENV_VAR}` syntax. When present, enables multi-tenancy. The LLM never sees or controls this value.
- **`tenant_column`** (string, default: `"user_id"`): The column name in tenant-isolated tables that holds the row owner. All RLS policies, defaults, and detection logic use this configured value — never hardcoded.
- **`auto_rls`** (boolean, default: `false`): Whether the node automatically creates RLS policies during initialization and after `CREATE TABLE` statements.

When `tenant_user_id` is **not set**, the node behaves exactly as today — no RLS, no SET, fully backwards compatible.

## How It Works

### 1. Query-Time Isolation (always active when `tenant_user_id` is set)

Before every query execution, the node runs:

```sql
SET app.current_user_id = '<resolved_tenant_user_id>';
```

PostgreSQL RLS policies reference `current_setting('app.current_user_id')` to filter rows. This is the only mechanism for isolation — the node does not parse or rewrite the LLM's SQL.

### 2. Auto-RLS Setup (when `auto_rls: true`)

#### 2.1 During `initialize()`

The node scans all tables in `allowed_schemas` and classifies them:

| Table state | Action |
|---|---|
| Has `tenant_column`, no RLS enabled | Enable RLS + create tenant isolation policy + add DEFAULT on tenant column |
| Has `tenant_column`, RLS already enabled | Skip (log: "already protected") |
| No `tenant_column`, no RLS enabled | Enable RLS + create read-only policy (SELECT only) |
| No `tenant_column`, RLS already enabled | Skip |

**Tenant isolation policy** (for tables with the tenant column):

```sql
ALTER TABLE <schema>.<table> ENABLE ROW LEVEL SECURITY;
CREATE POLICY colmena_tenant_isolation ON <schema>.<table>
  USING (<tenant_column> = current_setting('app.current_user_id'))
  WITH CHECK (<tenant_column> = current_setting('app.current_user_id'));
```

- `USING` filters SELECT, UPDATE, DELETE — user only sees own rows.
- `WITH CHECK` filters INSERT, UPDATE — user can only write rows with their own ID.

**Read-only policy** (for shared tables without the tenant column):

```sql
ALTER TABLE <schema>.<table> ENABLE ROW LEVEL SECURITY;
CREATE POLICY colmena_shared_read ON <schema>.<table>
  FOR SELECT USING (true);
-- No INSERT/UPDATE/DELETE policy = blocked by default when RLS is on
```

**Auto-DEFAULT** on the tenant column (so INSERTs don't need to include it):

```sql
ALTER TABLE <schema>.<table>
  ALTER COLUMN <tenant_column> SET DEFAULT current_setting('app.current_user_id');
```

This means the LLM can write `INSERT INTO todos (title) VALUES ('Buy milk')` and the tenant column is auto-filled with the current user's ID.

#### 2.2 After CREATE TABLE Detection

When the node detects a successful `CREATE TABLE` statement, it immediately applies the same logic before returning the result to the LLM:

1. Check if the new table has the `tenant_column`.
2. If yes: add DEFAULT on tenant column + enable RLS + create tenant isolation policy.
3. If no: enable RLS + create read-only policy.

If the table was created **without** the tenant column but `auto_rls` is on, the node auto-adds it:

```sql
ALTER TABLE <schema>.<table> ADD COLUMN <tenant_column> TEXT
  DEFAULT current_setting('app.current_user_id');
```

This guarantees no table ever exists unprotected when `auto_rls: true`.

#### 2.3 RLS Idempotency Check

Before applying policies, the node queries `pg_catalog` to check current state:

```sql
SELECT relrowsecurity FROM pg_class
WHERE relname = '<table>' AND relnamespace = '<schema_oid>';
```

If RLS is already enabled, the node skips with a log message. Policies use `IF NOT EXISTS` (PostgreSQL 15+) or are wrapped in a check against `pg_policies`.

### 3. When `auto_rls: false`

The node only runs `SET app.current_user_id` before each query. All RLS policies must be created by a DB admin beforehand. This is the recommended mode for production where the DB admin controls policies.

## Configuration Examples

### Minimal (prototyping with auto-setup)

```json
"permissions": {
  "preset": "read_write",
  "allowed_schemas": ["public"],
  "tenant_user_id": "${USER_ID}",
  "auto_rls": true
}
```

Uses default `tenant_column: "user_id"`. Node handles everything.

### Production (manual RLS, custom column)

```json
"permissions": {
  "preset": "read_write",
  "allowed_schemas": ["public"],
  "tenant_user_id": "${CURRENT_USER_ID}",
  "tenant_column": "owner_id",
  "auto_rls": false
}
```

DB admin has already created RLS policies referencing `owner_id`.

### Read-only analytics (multi-tenant)

```json
"permissions": {
  "preset": "read_only",
  "allowed_schemas": ["analytics"],
  "tenant_user_id": "${USER_ID}",
  "auto_rls": false
}
```

User can only SELECT, and only sees their own rows via pre-configured RLS.

### No multi-tenancy (backwards compatible)

```json
"permissions": {
  "preset": "read_only",
  "allowed_schemas": ["public"]
}
```

No `tenant_user_id` → no RLS, no SET. Behaves exactly as before.

## Test Graph Example

```json
{
  "nodes": {
    "todo_agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "${OPENAI_API_KEY}",
        "system_message": "You are a personal todo manager. Create, list, and manage the user's tasks.",
        "enabled_tools": ["manage_todos"],
        "tool_configurations": {
          "manage_todos": {
            "name": "manage_todos",
            "node_type": "sql_query",
            "description": "Manage the user's todo list in the database.",
            "node_schema": {
              "connection_url": {
                "type": "string",
                "fixed": "${DATABASE_URL_GRAPHS}"
              },
              "permissions": {
                "type": "object",
                "fixed": {
                  "preset": "read_write",
                  "allowed_schemas": ["public"],
                  "tenant_user_id": "${USER_ID}",
                  "tenant_column": "user_id",
                  "auto_rls": true
                }
              },
              "runtime_limits": {
                "type": "object",
                "fixed": {
                  "max_rows": 100,
                  "statement_timeout_ms": 15000
                }
              },
              "query": {
                "type": "string",
                "required": true,
                "description": "SQL query to manage todos"
              }
            }
          }
        },
        "prompt": "Show me my pending tasks"
      }
    },
    "result": {
      "type": "output",
      "config": { "label": "Todo Agent Result" }
    }
  },
  "edges": [
    { "from": "todo_agent", "to": "result" }
  ]
}
```

## Implementation Scope

### Changes to Existing Files

- **`sql_permissions.rs`**: Add `tenant_user_id`, `tenant_column`, `auto_rls` fields to `SqlPermissions`. Add methods `tenant_user_id()`, `tenant_column()`, `auto_rls()`.
- **`sql_ports.rs`**: Add `setup_rls()` and `check_rls_status()` methods to `SqlConnectionPort`. Add `auto_add_tenant_column()` method.
- **`sql_pool_adapter.rs`**: Implement the new port methods — query `pg_catalog` for RLS status, execute `ALTER TABLE`, `CREATE POLICY`.
- **`sql.rs` (SqlNode)**: In `initialize()`, after loading table metadata, run RLS setup if `auto_rls: true`. In `execute()`, prepend `SET app.current_user_id` when `tenant_user_id` is set. After `CREATE TABLE` detection, run RLS setup on the new table.
- **`sql_execution_service.rs`**: Add `set_tenant_context()` call before query execution. Add post-execution hook for `CREATE TABLE` detection.

### New Files

None — all changes fit within existing modules.

## Database Requirements

- PostgreSQL 9.5+ (RLS support)
- The connection role must **not** be a superuser (superusers bypass RLS)
- When `auto_rls: true`, the role needs `ALTER TABLE` privilege on tables in `allowed_schemas`
- When `auto_rls: false`, only `SELECT`/`INSERT`/`UPDATE`/`DELETE` and `SET` are needed

## Security Considerations

- `tenant_user_id` is resolved server-side via `${}` — the LLM cannot influence it
- RLS enforcement happens at the Postgres level — even if our code has bugs, Postgres won't leak rows
- `auto_rls: true` requires trust in the connection role's privilege level
- The `app.current_user_id` setting is session-scoped — concurrent queries from different users use separate connections from the pool, so there's no cross-contamination
- Shared tables (without tenant column) are automatically read-only when `auto_rls: true`

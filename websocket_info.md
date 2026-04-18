# ADP API Documentation

This document describes the complete API exposed by the ADP backend — both the REST API and the WebSocket (Socket.IO) real-time API. It covers authentication, all endpoints, namespaces, events, payloads, and infrastructure.

## Table of Contents

- [Stack](#stack)
- [Connection](#connection)
- [Authentication](#authentication)
- [REST API](#rest-api)
  - [Auth](#1-auth)
  - [User](#2-user)
  - [Workspace](#3-workspace)
  - [Project](#4-project)
  - [Folder](#5-folder)
  - [Environment](#6-environment)
  - [Nodes](#7-agents---nodes)
  - [Edges](#8-agents---edges)
  - [Groups](#9-agents---groups)
  - [Chat](#10-chat--agent-sessions)
  - [API Keys](#11-api-keys-provider-keys)
  - [RAG / File Stores](#12-rag--file-stores)
  - [Google Cloud Storage](#13-google-cloud-storage)
  - [Invitations](#14-invitations--members)
  - [Notifications](#15-notifications)
  - [Search](#16-search)
  - [Support](#17-support)
  - [Messages (Legacy)](#18-messages-legacy)
- [Node Reference](#node-reference)
- [WebSocket Namespaces](#namespaces)
  - [Canvas (`/canvas`)](#canvas-canvas)
  - [Project (`/project`)](#project-project)
  - [Speech (`/speech`)](#speech-speech)
- [Infrastructure](#infrastructure)
- [Environment Variables](#environment-variables)
- [Client Examples](#client-example)

---

## Stack

| Component          | Package                                             | Version   |
| ------------------ | --------------------------------------------------- | --------- |
| Server             | `socket.io`                                         | 4.8.3     |
| NestJS integration | `@nestjs/websockets` + `@nestjs/platform-socket.io` | 11.x      |
| Horizontal scaling | `@socket.io/redis-adapter` + `ioredis`              | 8.x / 5.x |
| Client             | `socket.io-client`                                  | 4.8.3     |

---

## Connection

**Base URL:** `http://localhost:8080` (configurable via `PORT` env var)

Each namespace is a separate Socket.IO connection appended to the base URL:

| Namespace | URL                  |
| --------- | -------------------- |
| Canvas    | `{BASE_URL}/canvas`  |
| Project   | `{BASE_URL}/project` |
| Speech    | `{BASE_URL}/speech`  |

**Recommended client options:**

```typescript
import { io } from 'socket.io-client';

const socket = io('http://localhost:8080/canvas', {
  withCredentials: true,               // send cookies for auth
  transports: ['websocket', 'polling'], // prefer websocket, fallback to polling
});
```

**CORS:** All namespaces accept any origin (`origin: true`) with `credentials: true`.

---

## Authentication

All WebSocket namespaces are protected by `WsSessionAuthGuard`. The guard validates a **BetterAuth session token** sent as a cookie during the Socket.IO handshake.

### How It Works (Internal Flow)

1. Client connects and includes the `cookie` header in the handshake.
2. The guard extracts the `cookie` header from `client.handshake.headers`.
3. Calls `authClient.api.getSession({ headers })` (BetterAuth) to validate the session.
4. On success, attaches `user` and `session` to the socket instance. Subsequent messages skip re-validation.
5. On failure, throws `WsException('Unauthorized')` and the connection is rejected.

### Obtaining a Session Token — Step by Step

External services (Python, Rust, curl, etc.) cannot use browser cookies automatically. You need to manually obtain a session token and then include it in every request. Here's how, step by step.

---

#### Step 1: Call the Login Endpoint

Make a `POST` request to `/api/auth/login` with your email and password:

```bash
curl -v -X POST http://localhost:8080/api/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email": "user@example.com", "password": "your_password"}'
```

#### Step 2: Read the Response

The login endpoint returns the token in **two places** — you can use either one:

**Place 1 — Response body** (JSON):

```json
{
  "token": "abc123xyz...",
  "user": {
    "id": "clx...",
    "email": "user@example.com",
    "name": "John Doe",
    "image": null
  }
}
```

The `token` field at the root of the response **is your session token**.

**Place 2 — `set-cookie` response header**:

```
set-cookie: better-auth.session_token=abc123xyz...; Path=/; HttpOnly; SameSite=Lax
```

The value after `better-auth.session_token=` and before the first `;` **is the same token**.

> Both values are identical. Use whichever is easier for your language/library.

#### Step 3: Use the Token in REST API Calls

For any authenticated REST endpoint, include the token as a **Bearer token** in the `Authorization` header:

```bash
curl http://localhost:8080/user/me \
  -H "Authorization: Bearer abc123xyz..."
```

Or as a **cookie** (also valid for REST):

```bash
curl http://localhost:8080/user/me \
  -H "Cookie: better-auth.session_token=abc123xyz..."
```

Both methods work for REST endpoints. The `SessionAuthGuard` accepts either format.

#### Step 4: Use the Token in WebSocket Connections

For WebSocket (Socket.IO), the token **must** be sent as a cookie in the handshake headers. The `Authorization: Bearer` header is **NOT supported** by the WebSocket guard — only cookies.

```
Cookie: better-auth.session_token=abc123xyz...
```

---

#### Complete Example: curl

```bash
# 1. Login and extract the token from the JSON response
TOKEN=$(curl -s -X POST http://localhost:8080/api/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email": "user@example.com", "password": "your_password"}' \
  | python3 -c "import sys,json; print(json.load(sys.stdin)['token'])")

echo "Token: $TOKEN"

# 2. Use the token for REST API calls
curl -s http://localhost:8080/user/me \
  -H "Authorization: Bearer $TOKEN" | python3 -m json.tool

# 3. List workspaces
curl -s http://localhost:8080/workspace \
  -H "Authorization: Bearer $TOKEN" | python3 -m json.tool

# 4. List projects in a workspace
curl -s "http://localhost:8080/projects?workspaceId=YOUR_WORKSPACE_ID" \
  -H "Authorization: Bearer $TOKEN" | python3 -m json.tool
```

#### Complete Example: Python

```python
import requests

API_URL = "http://localhost:8080"

# 1. Login
response = requests.post(f"{API_URL}/api/auth/login", json={
    "email": "user@example.com",
    "password": "your_password",
})

# Method A: Extract token from response body
data = response.json()
token = data["token"]

# Method B: Extract token from cookies (alternative)
# token = response.cookies.get("better-auth.session_token")

print(f"Token: {token[:30]}...")

# 2. Use the token for REST API calls (Bearer header)
headers = {"Authorization": f"Bearer {token}"}

me = requests.get(f"{API_URL}/user/me", headers=headers).json()
print(f"Logged in as: {me['name']} ({me['email']})")

workspaces = requests.get(f"{API_URL}/workspace", headers=headers).json()
print(f"Workspaces: {[w['name'] for w in workspaces]}")

workspace_id = workspaces[0]["id"]
projects = requests.get(f"{API_URL}/projects", headers=headers, params={
    "workspaceId": workspace_id,
}).json()
print(f"Projects: {[p['name'] for p in projects]}")

# 3. Use the same token for WebSocket (as cookie)
import socketio

sio = socketio.Client()
cookie = f"better-auth.session_token={token}"

@sio.on("canvas_state_loaded", namespace="/canvas")
def on_state(data):
    print(f"Loaded {len(data['nodes'])} nodes")

sio.connect(API_URL, namespaces=["/canvas"], headers={"Cookie": cookie})
sio.emit("load_canvas_state", {"environmentId": "YOUR_ENV_ID"}, namespace="/canvas")
sio.wait()
```

#### Complete Example: Rust

```rust
use reqwest::blocking::Client;
use serde_json::{json, Value};

fn main() {
    let api_url = "http://localhost:8080";

    // 1. Login
    let client = Client::new();
    let res = client
        .post(format!("{api_url}/api/auth/login"))
        .json(&json!({
            "email": "user@example.com",
            "password": "your_password",
        }))
        .send()
        .expect("Login request failed");

    let body: Value = res.json().expect("Failed to parse login response");
    let token = body["token"]
        .as_str()
        .expect("No token in response");

    println!("Token: {}...", &token[..30]);

    // 2. Use for REST API calls (Bearer header)
    let me: Value = client
        .get(format!("{api_url}/user/me"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .expect("Request failed")
        .json()
        .expect("Parse failed");

    println!("Logged in as: {}", me["name"]);

    // 3. Use for WebSocket (as cookie)
    let session_cookie = format!("better-auth.session_token={token}");

    let _socket = rust_socketio::ClientBuilder::new(api_url)
        .namespace("/canvas")
        .opening_header("Cookie", &session_cookie)
        .transport_type(rust_socketio::TransportType::Websocket)
        .on("canvas_state_loaded", |payload, _client, _| {
            println!("Canvas loaded: {:?}", payload);
        })
        .connect()
        .expect("WebSocket connection failed");
}
```

---

### Where to Use the Token — Summary

| Context                | How to Send Token               | Example Header                                |
| ---------------------- | ------------------------------- | --------------------------------------------- |
| REST API               | `Authorization: Bearer <TOKEN>` | `Authorization: Bearer abc123...`             |
| REST API (alternative) | `Cookie` header                 | `Cookie: better-auth.session_token=abc123...` |
| WebSocket (Socket.IO)  | `Cookie` header **only**        | `Cookie: better-auth.session_token=abc123...` |

> **Key difference:** REST endpoints accept both `Authorization: Bearer` and `Cookie`. WebSocket **only** accepts `Cookie`. If you're getting `Unauthorized` on WebSocket, make sure you're sending the token as a cookie, not as a Bearer header.

### Token Lifecycle

- Tokens are stored in the `session` table in PostgreSQL.
- Each session has an `expiresAt` field. Expired tokens are rejected.
- To refresh, call `POST /api/auth/login` again to get a new token.
- There is no dedicated token refresh endpoint — re-login is required when the session expires.
- If you get a `401 Unauthorized` on a previously working token, it has expired — login again.

### Troubleshooting

| Problem                               | Cause                                           | Solution                                                                                  |
| ------------------------------------- | ----------------------------------------------- | ----------------------------------------------------------------------------------------- |
| Login returns 401                     | Wrong email or password                         | Verify credentials, register a new user if needed                                         |
| REST returns 401 with valid token     | Token expired                                   | Call `/api/auth/login` again                                                              |
| REST returns 401                      | Missing `Bearer ` prefix                        | Header must be `Authorization: Bearer <token>`, not just `Authorization: <token>`         |
| WebSocket disconnects immediately     | Token sent as Bearer, not Cookie                | Use `Cookie: better-auth.session_token=<token>` in handshake headers                      |
| WebSocket `Unauthorized`              | No cookies in handshake                         | Ensure your Socket.IO client sends the `Cookie` header (some clients strip it by default) |
| `set-cookie` header not visible       | HTTP library follows redirects or hides cookies | Read the response body `token` field instead                                              |
| Token works in REST but not WebSocket | Different token formats                         | Use the exact same token string for both — no encoding, no quotes                         |

---

## REST API

**Base URL:** `http://localhost:8080`  
**Swagger Docs:** `http://localhost:8080/api`  
**Global Validation:** All request bodies are validated with `class-validator` (whitelist mode, unknown properties rejected).

Most endpoints require authentication via session cookie or Bearer token:

```
Authorization: Bearer <session-token>
```

or

```
Cookie: better-auth.session_token=<session-token>
```

---

### 1. Auth

**Base route:** `api/auth`

#### Register

```
POST /api/auth/register
```

| Field      | Type   | Required | Validation       |
| ---------- | ------ | -------- | ---------------- |
| `name`     | string | yes      | non-empty        |
| `email`    | string | yes      | valid email      |
| `password` | string | yes      | min 8 characters |

**Response:** Session object with user info and token. Sets `better-auth.session_token` cookie.

#### Login

```
POST /api/auth/login
```

| Field      | Type   | Required | Validation  |
| ---------- | ------ | -------- | ----------- |
| `email`    | string | yes      | valid email |
| `password` | string | yes      | non-empty   |

**Response:** Session object with user info and token. Sets `better-auth.session_token` cookie.

#### Get Current Session

```
GET /api/auth/session
```

**Auth:** Optional (returns `{ user: null, session: null }` if unauthenticated)

**Response:**
```json
{
  "user": { "id": "...", "email": "...", "name": "...", "image": "..." },
  "session": { "token": "...", "expiresAt": "..." }
}
```

#### Forgot Password

```
POST /api/auth/forgot-password
```

| Field        | Type   | Required | Description                 |
| ------------ | ------ | -------- | --------------------------- |
| `email`      | string | yes      | User email                  |
| `redirectTo` | string | yes      | URL to redirect after reset |

#### Reset Password

```
POST /api/auth/reset-password
```

| Field         | Type   | Required | Validation             |
| ------------- | ------ | -------- | ---------------------- |
| `newPassword` | string | yes      | min 8 characters       |
| `token`       | string | yes      | Token from reset email |

#### Change Password

```
POST /api/auth/change-password
```

**Auth:** Required

| Field             | Type   | Required | Validation       |
| ----------------- | ------ | -------- | ---------------- |
| `currentPassword` | string | yes      | non-empty        |
| `newPassword`     | string | yes      | min 8 characters |

#### Check User Exists

```
GET /api/auth/user-exists?email=user@example.com
```

**Response:** `{ "exists": true }`

#### Debug Session (Development)

```
GET /api/auth/debug-session/:token
```

Returns detailed session debug info from BetterAuth and direct DB lookup.

---

### 2. User

**Base route:** `user` — **Auth:** Required

#### Get Current User

```
GET /user/me
```

**Response:**
```json
{
  "id": "...",
  "name": "John Doe",
  "email": "john@example.com",
  "image": "https://...",
  "isPlatformAdmin": false,
  "createdAt": "...",
  "updatedAt": "..."
}
```

#### Get User by ID

```
GET /user/:id
```

#### Update Current User

```
PATCH /user/update
```

| Field   | Type   | Required |
| ------- | ------ | -------- |
| `name`  | string | no       |
| `image` | string | no       |

**Response:** `{ "success": true, "user": {...} }`

#### Delete Current User

```
DELETE /user/delete
```

**Response:** `{ "success": true }`

---

### 3. Workspace

**Base route:** `workspace` — **Auth:** Required

#### List Workspaces

```
GET /workspace
```

**Response:** Array of workspaces the user belongs to.

```json
[
  {
    "id": "...",
    "name": "My Workspace",
    "color": "#ff0000",
    "image": "https://...",
    "role": "OWNER",
    "createdAt": "...",
    "updatedAt": "..."
  }
]
```

#### Create Workspace

```
POST /workspace
```

| Field   | Type   | Required |
| ------- | ------ | -------- |
| `name`  | string | yes      |
| `color` | string | no       |
| `type`  | string | no       |
| `image` | string | no       |

#### Update Workspace

```
PATCH /workspace
```

**Permission:** Workspace owner only.

| Field   | Type   | Required |
| ------- | ------ | -------- |
| `id`    | string | yes      |
| `name`  | string | no       |
| `color` | string | no       |
| `image` | string | no       |

#### Delete Workspace

```
DELETE /workspace/:id
```

**Permission:** Workspace owner only.

#### List Members

```
GET /workspace/members?workspaceId=<id>
```

#### Update Member Role

```
PATCH /workspace/members/update
```

| Field         | Type   | Required | Description                                            |
| ------------- | ------ | -------- | ------------------------------------------------------ |
| `workspaceId` | string | yes      |                                                        |
| `memberId`    | string | yes      |                                                        |
| `role`        | string | yes      | `OWNER`, `BUILDER_GLOBAL`, `BUILDER_GUEST`, `CONSUMER` |

#### Remove Members

```
DELETE /workspace/members/delete
```

| Field         | Type     | Required |
| ------------- | -------- | -------- |
| `workspaceId` | string   | yes      |
| `memberIds`   | string[] | yes      |

#### Leave Workspace

```
POST /workspace/leave
```

| Field         | Type   | Required |
| ------------- | ------ | -------- |
| `workspaceId` | string | yes      |

---

### 4. Project

**Base route:** `projects` — **Auth:** Required

#### List Projects

```
GET /projects?workspaceId=<id>&folderId=<id>
```

| Query Param   | Type   | Required |
| ------------- | ------ | -------- |
| `workspaceId` | string | yes      |
| `folderId`    | string | no       |

**Response:**
```json
[
  {
    "id": "...",
    "name": "My Project",
    "description": null,
    "image": null,
    "starred": false,
    "workspaceId": "...",
    "folderId": null,
    "createdAt": "...",
    "updatedAt": "...",
    "createdBy": "...",
    "updatedBy": "...",
    "updatedByName": "John Doe"
  }
]
```

#### List Shared Projects

```
GET /projects/shared
```

Returns projects shared with the user across all workspaces.

#### Get Project

```
GET /projects/:id
```

#### Create Project

```
POST /projects
```

**Permission:** Requires `OWNER` or `BUILDER_GLOBAL` role in the workspace.

| Field         | Type   | Required |
| ------------- | ------ | -------- |
| `name`        | string | yes      |
| `workspaceId` | string | yes      |
| `folderId`    | string | no       |

#### Update Project

```
PATCH /projects/:id
```

| Field      | Type   | Required |
| ---------- | ------ | -------- |
| `name`     | string | no       |
| `folderId` | string | no       |

#### Delete Project

```
DELETE /projects/:id
```

**Permission:** Workspace owner only.

---

### 5. Folder

**Base route:** `folders` — **Auth:** Required

#### List Folders

```
GET /folders?workspaceId=<id>&order=asc
```

| Query Param   | Type            | Required | Default |
| ------------- | --------------- | -------- | ------- |
| `workspaceId` | string          | yes      |         |
| `order`       | `asc` \| `desc` | no       | `asc`   |

#### Get Folder

```
GET /folders/:id
```

#### Create Folder

```
POST /folders
```

| Field         | Type   | Required |
| ------------- | ------ | -------- |
| `name`        | string | yes      |
| `workspaceId` | string | yes      |

#### Update Folder

```
PATCH /folders/:id
```

| Field  | Type   | Required |
| ------ | ------ | -------- |
| `name` | string | no       |

#### Delete Folder

```
DELETE /folders/:id
```

---

### 6. Environment

**Base route:** `environments` — **Auth:** Required

#### List Environments

```
GET /environments?projectId=<id>
```

| Query Param | Type   | Required |
| ----------- | ------ | -------- |
| `projectId` | string | yes      |

**Response:**
```json
[
  {
    "id": "...",
    "name": "Production",
    "projectId": "...",
    "position": 0,
    "createdAt": "...",
    "updatedAt": "..."
  }
]
```

#### Get Environment

```
GET /environments/:id
```

#### Create Environment

```
POST /environments
```

| Field       | Type   | Required |
| ----------- | ------ | -------- |
| `name`      | string | yes      |
| `projectId` | string | yes      |

#### Update Environment

```
PATCH /environments/:id
```

| Field       | Type   | Required |
| ----------- | ------ | -------- |
| `name`      | string | no       |
| `projectId` | string | no       |

#### Delete Environment

```
DELETE /environments/:id
```

---

### 7. Agents - Nodes

**Base route:** `agents/nodes`

#### List Nodes

```
GET /agents/nodes
```

#### Get Node

```
GET /agents/nodes/:id
```

#### Create Node

```
POST /agents/nodes
```

| Field           | Type                | Required | Example                                      |
| --------------- | ------------------- | -------- | -------------------------------------------- |
| `type`          | string              | yes      | `"triggerNode"`                              |
| `category`      | string              | yes      | `"trigger"`                                  |
| `environmentId` | string              | yes      |                                              |
| `position`      | `{ x, y }`          | yes      | `{ "x": 100, "y": 200 }`                     |
| `data`          | object              | yes      | `{ "label": "Chat Input", "config": {...} }` |
| `measured`      | `{ width, height }` | no       | `{ "width": 189, "height": 50 }`             |
| `groupId`       | string \| null      | no       |                                              |
| `sortIndex`     | number              | no       |                                              |

> See the [Node Reference](#node-reference) section for all node types and their `data.config` schemas.

#### Update Node

```
PATCH /agents/nodes/:id
```

All fields from CreateNode are optional.

#### Delete Node

```
DELETE /agents/nodes/:id
```

---

### 8. Agents - Edges

**Base route:** `agents/edges`

#### List Edges

```
GET /agents/edges
```

#### Get Edge

```
GET /agents/edges/:id
```

#### Create Edge

```
POST /agents/edges
```

| Field           | Type    | Required | Description                  |
| --------------- | ------- | -------- | ---------------------------- |
| `id`            | string  | no       | Auto-generated if omitted    |
| `animated`      | boolean | yes      | Animate the edge line        |
| `type`          | string  | yes      | Edge type (e.g. `"default"`) |
| `environmentId` | string  | yes      |                              |
| `source`        | string  | yes      | Source node ID               |
| `sourceHandle`  | string  | no       | Source handle ID             |
| `target`        | string  | yes      | Target node ID               |
| `targetHandle`  | string  | no       | Target handle ID             |
| `groupId`       | string  | no       | Group to assign the edge to  |

#### Update Edge

```
PATCH /agents/edges/:id
```

All fields from CreateEdge are optional.

#### Delete Edge

```
DELETE /agents/edges/:id
```

---

### 9. Agents - Groups

**Base route:** `agents/groups` — **Auth:** Required

#### List Groups

```
GET /agents/groups?environmentId=<id>
```

#### Get Accessible Groups

```
GET /agents/groups/accessible?workspaceId=<id>
```

Returns all groups the user can access in the given workspace (includes published groups from other workspaces based on permission levels).

#### Get Group

```
GET /agents/groups/:id
```

#### Get Group by Custom ID

```
GET /agents/groups/custom/:customId
```

#### Get Group Config (Public)

```
GET /agents/groups/:id/config
```

**Auth:** Not required. Returns the compiled agent configuration JSON.

#### Create Group

```
POST /agents/groups
```

| Field               | Type   | Required | Default     | Description                                |
| ------------------- | ------ | -------- | ----------- | ------------------------------------------ |
| `environmentId`     | string | yes      |             |                                            |
| `id`                | string | no       | auto        | Custom ID                                  |
| `name`              | string | no       |             | Group name                                 |
| `label`             | string | no       |             | Group label                                |
| `sortIndex`         | number | no       | `0`         | Display order                              |
| `description`       | string | no       |             |                                            |
| `publishStatus`     | string | no       | `"DRAFT"`   | `DRAFT` or `PUBLISHED`                     |
| `publishPermission` | string | no       | `"PRIVATE"` | `PRIVATE`, `WORKSPACE`, `PUBLIC`, `GLOBAL` |
| `customId`          | string | no       |             | For `GLOBAL` permission (admin only)       |

#### Update Group

```
PATCH /agents/groups/:id
```

All fields from CreateGroup are optional. Setting `publishPermission` to `GLOBAL` requires platform admin.

#### Delete Group

```
DELETE /agents/groups/:id
```

#### Compile All Groups

```
POST /agents/groups/compile?environmentId=<id>
```

Compiles agent graphs for all groups in the environment.

#### Add/Remove Nodes and Edges to Group

```
POST   /agents/groups/:id/nodes         { "nodeId": "..." }
DELETE /agents/groups/:id/nodes/:nodeId

POST   /agents/groups/:id/edges         { "edgeId": "..." }
DELETE /agents/groups/:id/edges/:edgeId
```

---

### 10. Chat / Agent Sessions

**Base route:** `chat` — **Auth:** Required

#### Stream Chat (SSE)

```
POST /chat/stream
```

Server-Sent Events endpoint for streaming agent responses.

| Field         | Type   | Required | Description            |
| ------------- | ------ | -------- | ---------------------- |
| `messages`    | array  | yes      | Chat messages          |
| `sessionId`   | string | yes      | Agent session ID       |
| `groupId`     | string | no       | Agent group to use     |
| `origin`      | string | no       | `"BUILDER"` or `"APP"` |
| `workspaceId` | string | no       |                        |

**Response:** SSE stream with chunked agent responses.

#### List Sessions

```
GET /chat/sessions?groupId=<id>&origin=<origin>&workspaceId=<id>&search=<query>
```

All query params are optional. Returns agent sessions for the authenticated user.

**Response:**
```json
[
  {
    "id": "...",
    "title": "My Chat",
    "userId": "...",
    "groupId": "...",
    "workspaceId": "...",
    "origin": "BUILDER",
    "pinned": false,
    "isSuspended": false,
    "createdAt": "...",
    "updatedAt": "..."
  }
]
```

#### Create Session

```
POST /chat/sessions
```

| Field   | Type   | Required |
| ------- | ------ | -------- |
| `title` | string | yes      |

#### Get Session Messages

```
GET /chat/sessions/:id/messages
```

**Response:**
```json
[
  {
    "id": "...",
    "sessionId": "...",
    "role": "user",
    "content": "Hello",
    "feedback": null,
    "attachments": [],
    "createdAt": "..."
  }
]
```

#### Get Session Status

```
GET /chat/sessions/:id/status
```

#### Delete Session

```
DELETE /chat/sessions/:id
```

#### Rename Session

```
POST /chat/sessions/:id/rename
```

| Field   | Type   | Required |
| ------- | ------ | -------- |
| `title` | string | yes      |

#### Pin/Unpin Session

```
POST /chat/sessions/:id/pin
```

| Field    | Type    | Required |
| -------- | ------- | -------- |
| `pinned` | boolean | yes      |

#### Update Session Group

```
POST /chat/sessions/:id/group
```

| Field     | Type   | Required |
| --------- | ------ | -------- |
| `groupId` | string | yes      |

#### Message Feedback

```
POST /chat/sessions/:sessionId/messages/:messageId/feedback
```

| Field      | Type           | Required | Description                                    |
| ---------- | -------------- | -------- | ---------------------------------------------- |
| `feedback` | string \| null | yes      | `"positive"`, `"negative"`, or `null` to clear |

#### Undo Messages

```
DELETE /chat/sessions/:sessionId/messages/:messageId/undo
```

Deletes the target message and all messages after it in the session.

#### Legacy Chat Endpoints

```
GET  /chat?workspaceId=<id>     — List workspace chats
GET  /chat/:id                  — Get chat by ID
POST /chat                      — Create chat
```

---

### 11. API Keys (Provider Keys)

**Base route:** `api-keys` — **Auth:** Required

#### List API Keys

```
GET /api-keys?workspaceId=<id>&projectId=<id>
```

| Query Param   | Type   | Required |
| ------------- | ------ | -------- |
| `workspaceId` | string | yes      |
| `projectId`   | string | no       |

**Response:**
```json
[
  {
    "id": "...",
    "name": "My OpenAI Key",
    "provider": "openai",
    "textSecret": "sk-...****",
    "permission": "WORKSPACE",
    "projectId": null,
    "workspaceId": "...",
    "createdBy": "...",
    "createdAt": "...",
    "updatedAt": "..."
  }
]
```

#### Create API Key

```
POST /api-keys
```

| Field         | Type   | Required | Description                                           |
| ------------- | ------ | -------- | ----------------------------------------------------- |
| `name`        | string | yes      | Display name                                          |
| `provider`    | string | yes      | Provider (e.g. `"openai"`, `"google"`, `"anthropic"`) |
| `secretKey`   | string | yes      | The actual API key                                    |
| `permission`  | string | yes      | `"WORKSPACE"` or `"PROJECT"`                          |
| `projectId`   | string | no       | Required if permission is `PROJECT`                   |
| `workspaceId` | string | yes      |                                                       |

#### Update API Key

```
PATCH /api-keys/:id?workspaceId=<id>
```

All fields from Create are optional.

#### Reveal API Key

```
GET /api-keys/:id/reveal?workspaceId=<id>
```

**Response:** `{ "secretKey": "sk-actual-decrypted-key..." }`

#### List Available Models

```
GET /api-keys/:id/models?workspaceId=<id>
```

**Response:** Array of model name strings available for this provider key.

#### Delete API Key

```
DELETE /api-keys/:id?workspaceId=<id>
```

---

### 12. RAG / File Stores

**Base route:** `rag`

#### Create File Store

```
POST /rag/filestores
```

| Field       | Type   | Required | Description                |
| ----------- | ------ | -------- | -------------------------- |
| `name`      | string | yes      | e.g. `"My Knowledge Base"` |
| `projectId` | string | yes      |                            |

#### List File Stores

```
GET /rag/projects/:projectId/filestores
```

#### Get File Store

```
GET /rag/filestores/:id
```

#### Delete File Store

```
DELETE /rag/filestores/:id
```

#### Upload File

```
POST /rag/filestores/:id/files
Content-Type: multipart/form-data
```

Send a file via multipart form upload. The file is processed and indexed for RAG queries.

#### List Files

```
GET /rag/filestores/:id/files
```

#### Delete File

```
DELETE /rag/files/:id
```

#### Query File Store

```
POST /rag/filestores/:id/query
```

| Field   | Type   | Required |
| ------- | ------ | -------- |
| `query` | string | yes      |

**Response:** Array of relevant document chunks with similarity scores.

---

### 13. Google Cloud Storage

**Base route:** `gcs`

#### Get Upload URL

```
GET /gcs/upload-url?fileName=<name>&contentType=<mime>&path=<path>
```

| Query Param   | Type   | Required | Default |
| ------------- | ------ | -------- | ------- |
| `fileName`    | string | yes      |         |
| `contentType` | string | yes      |         |
| `path`        | string | no       | `""`    |

**Allowed content types:** `image/png`, `image/jpeg`, `image/jpg`, `image/webp`, `application/pdf`, `application/vnd.openxmlformats-officedocument.wordprocessingml.document`, `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet`, `text/csv`, `text/plain`

**Response:** `{ "signedUrl": "https://storage.googleapis.com/...", "bucketName": "..." }`

#### Get Read URL

```
GET /gcs/read-url?fileName=<name>&path=<path>
```

**Response:** `{ "signedUrl": "https://storage.googleapis.com/..." }`

#### Delete File

```
DELETE /gcs/file?fileName=<name>&path=<path>
```

**Response:** `{ "success": true }`

---

### 14. Invitations & Members

**Base route:** `invite` — **Auth:** Required (except `info/:token`)

#### Create Invite Link

```
POST /invite/create
```

| Field         | Type   | Required |
| ------------- | ------ | -------- |
| `workspaceId` | string | yes      |

#### Send Workspace Invites

```
POST /invite/send
```

| Field         | Type   | Required | Description                                      |
| ------------- | ------ | -------- | ------------------------------------------------ |
| `workspaceId` | string | yes      |                                                  |
| `invites`     | array  | yes      | `[{ "email": "...", "role": "BUILDER_GLOBAL" }]` |
| `emailText`   | string | no       | Custom email message                             |

**Response:** `{ "results": [{ "email": "...", "status": "sent" }] }`

#### Send Project Invites

```
POST /invite/send-project
```

| Field       | Type     | Required |
| ----------- | -------- | -------- |
| `projectId` | string   | yes      |
| `emails`    | string[] | yes      |
| `role`      | string   | no       |
| `emailText` | string   | no       |

#### Send Agent Invites

```
POST /invite/send-agent
```

| Field       | Type     | Required |
| ----------- | -------- | -------- |
| `groupId`   | string   | yes      |
| `emails`    | string[] | yes      |
| `role`      | string   | no       |
| `emailText` | string   | no       |

#### Send Admin Invitation

```
POST /invite/send-admin
```

| Field   | Type   | Required |
| ------- | ------ | -------- |
| `email` | string | yes      |

#### Get Invite Info (Public)

```
GET /invite/info/:token
```

**Auth:** Not required. Returns invite details for the given token.

#### Accept Invite

```
POST /invite/accept
```

| Field   | Type   | Required |
| ------- | ------ | -------- |
| `token` | string | yes      |

#### Decline Invite

```
POST /invite/decline
```

| Field   | Type   | Required |
| ------- | ------ | -------- |
| `token` | string | yes      |

#### Delete Invitations

```
DELETE /invite/delete
```

| Field | Type     | Required |
| ----- | -------- | -------- |
| `ids` | string[] | yes      |

#### Resend Invite

```
POST /invite/resend
```

| Field       | Type   | Required |
| ----------- | ------ | -------- |
| `id`        | string | yes      |
| `emailText` | string | no       |

#### Update Invitation Role

```
PATCH /invite/update
```

| Field          | Type   | Required |
| -------------- | ------ | -------- |
| `invitationId` | string | yes      |
| `role`         | string | yes      |

#### Project Members

```
GET    /invite/project/:projectId/members                          — List members
GET    /invite/project/:projectId/invitations                      — List pending invitations
DELETE /invite/project/:projectId/member/:memberId                 — Remove member
PATCH  /invite/project/:projectId/member/:memberId/role            — Update role
       Body: { "role": "..." }
```

#### Agent Access

```
GET    /invite/agent/:groupId/access                               — List access entries
GET    /invite/agent/:groupId/invitations                          — List pending invitations
DELETE /invite/agent/:groupId/access/:accessId                     — Remove access
```

---

### 15. Notifications

**Base route:** `notifications` — **Auth:** Required

#### List Notifications

```
GET /notifications
```

**Response:**
```json
{
  "notifications": [
    {
      "id": "...",
      "type": "WORKSPACE_INVITE",
      "message": "You've been invited to...",
      "url": "/workspace/...",
      "read": false,
      "createdAt": "...",
      "fromUser": { "name": "John", "image": "..." }
    }
  ]
}
```

#### Mark as Read

```
PUT /notifications/:id/read
```

#### Bulk Mark as Read

```
PATCH /notifications/bulk-read
```

| Field | Type     | Required |
| ----- | -------- | -------- |
| `ids` | string[] | yes      |

#### Bulk Delete

```
DELETE /notifications/bulk-delete
```

| Field | Type     | Required |
| ----- | -------- | -------- |
| `ids` | string[] | yes      |

#### Notification Settings

```
GET   /notifications/settings?workspaceId=<id>
PATCH /notifications/settings/update
      Body: { "workspaceId": "...", ...settings }
```

---

### 16. Search

**Base route:** `search` — **Auth:** Required

```
GET /search?workspaceId=<id>&q=<query>
```

| Query Param   | Type   | Required |
| ------------- | ------ | -------- |
| `workspaceId` | string | yes      |
| `q`           | string | yes      |

Searches across projects, environments, agents, and other resources in the workspace.

---

### 17. Support

**Base route:** `support`

#### Create Ticket

```
POST /support/ticket
```

| Field     | Type   | Required |
| --------- | ------ | -------- |
| `subject` | string | yes      |
| `message` | string | yes      |

#### List Tickets

```
GET /support/tickets
```

---

### 18. Messages (Legacy)

**Base route:** `message`

```
GET  /message?chatId=<id>    — Get messages for a legacy chat
POST /message                 — Create a message in a legacy chat
```

---

### REST API — Data Model Reference

#### Workspace Roles

| Role             | Description                          |
| ---------------- | ------------------------------------ |
| `OWNER`          | Full access, can delete workspace    |
| `BUILDER_GLOBAL` | Can create projects and agents       |
| `BUILDER_GUEST`  | Can edit shared projects only        |
| `CONSUMER`       | Read-only access to published agents |

#### Publish Status

| Value       | Description                |
| ----------- | -------------------------- |
| `DRAFT`     | Agent is in development    |
| `PUBLISHED` | Agent is available for use |

#### Publish Permission

| Value       | Description                      |
| ----------- | -------------------------------- |
| `PRIVATE`   | Only visible to the creator      |
| `WORKSPACE` | Visible to all workspace members |
| `PUBLIC`    | Visible to anyone with the link  |
| `GLOBAL`    | Platform-wide (admin only)       |

#### Agent Session Origin

| Value     | Description                       |
| --------- | --------------------------------- |
| `BUILDER` | Session from the canvas editor    |
| `APP`     | Session from the chat application |

---

## Node Reference

Nodes are the building blocks of the canvas. Each node has a `type`, a `category`, a `position`, and a `data` object containing its configuration. This section documents every available node type so you can create them via the `create_node` WebSocket event.

### CreateNode Payload Structure

```typescript
{
  environmentId: string;          // ID of the target environment
  node: {
    id?: string;                  // Optional — auto-generated if omitted
    type: string;                 // Node type ID (see tables below)
    category: string;             // Node category (see mapping below)
    position: { x: number; y: number };  // Canvas coordinates
    data: {
      label: string;              // Display name on the canvas
      config?: object;            // Type-specific configuration (see defaults below)
    };
    measured?: { width: number; height: number };  // Optional layout size
    groupId?: string | null;      // Optional — assign to an existing group
    sortIndex?: number;           // Optional — display order within a group
  }
}
```

### Type-to-Category Mapping

Every node type belongs to exactly one category. The `category` field is **required** when creating a node.

| Category  | Type IDs                                                         |
| --------- | ---------------------------------------------------------------- |
| `trigger` | `chatInput`, `webhook`, `schedule`                               |
| `ai`      | `llmCall`, `agent`, `planner`, `orchestrator`, `agentTeam`       |
| `tools`   | `webSearch`, `databaseQuery`, `fileSearch`, `apiCall`, `runCode` |
| `logic`   | `condition`, `memory`, `humanInTheLoop`, `parser`                |
| `output`  | `chatOutput`, `fileOutput`, `objectOutput`                       |

### Connection Rules

When edges are created between nodes, the system enforces these rules:

| Source Category                  | Can Connect To                                 |
| -------------------------------- | ---------------------------------------------- |
| `trigger`                        | `ai` only                                      |
| `ai`                             | `ai`, `condition`, `logic`, `output`, `memory` |
| `tools`                          | Cannot output (input-only nodes)               |
| `logic` (condition)              | Anything except `trigger`                      |
| `logic` (memory)                 | Cannot output (input-only nodes)               |
| `logic` (parser, humanInTheLoop) | Anything except `trigger`                      |
| `output`                         | Cannot output (terminal nodes)                 |

### Handle Behavior (sourceHandle / targetHandle)

When creating edges via WebSocket (`create_edge`), you can specify `sourceHandle` and `targetHandle` to indicate which connection point on the node to use. Each handle has an ID that matches its position: `top`, `bottom`, `left`, `right`.

Handles have two types:

- **source** (output, green) — the edge starts here
- **target** (input, purple) — the edge arrives here

The type of each handle **changes dynamically** depending on how many connections a node already has. This section documents the exact behavior so you know which handles are available for programmatic edge creation.

#### Handle Positions

```
            ┌─── top ───┐
            │            │
   left ────┤    Node    ├──── right
            │            │
            └── bottom ──┘
```

#### Trigger Nodes (`chatInput`, `webhook`, `schedule`)

**All handles are SOURCE (output).** Trigger nodes only emit — they never receive input.

| Handle   | Type   | Direction |
| -------- | ------ | --------- |
| `bottom` | source | output    |
| `right`  | source | output    |
| `left`   | source | output    |

- `top` handle does not exist on trigger nodes.
- **Hide rule:** Once any handle is connected, unconnected handles are hidden. Only connected handles remain visible.
- **Validation:** Can only connect to nodes with `category: 'ai'`.

#### LLM Node (`llmCall`)

**Handles change type dynamically based on connection count.** All 4 positions exist.

| Connections      | `top`                                                | `bottom`    | `left`              | `right` |
| ---------------- | ---------------------------------------------------- | ----------- | ------------------- | ------- |
| 0 (initial)      | target                                               | target      | target              | target  |
| 1 input          | *connected*                                          | source      | source              | source  |
| 2 (1 in + 1 out) | *connected*                                          | *connected* | target (restricted) | source  |
| 3+               | balanced — extra handles split between target/source |

> The exact assignment depends on which handles get connected first. The table shows one example; the logic always ensures a mix of inputs and outputs.

- **Hide rule:** When 3+ connections exist and 2+ are inputs, unconnected handles are hidden.
- **Restricted target validation:** Side handles acting as restricted targets only accept connections from `ai`, `condition`, or `logic` nodes.
- **Source validation:** Can connect to `ai`, `condition`, `logic`, `output`, `memory` nodes.

#### Agent Node (`agent`)

**Identical dynamic behavior to LLM Node** — same state machine for handle type assignment.

| Connections      | `top`       | `bottom`    | `left`              | `right` |
| ---------------- | ----------- | ----------- | ------------------- | ------- |
| 0 (initial)      | target      | target      | target              | target  |
| 1 input          | *connected* | source      | source              | source  |
| 2 (1 in + 1 out) | *connected* | *connected* | target (restricted) | source  |
| 3+               | balanced    |

- **Hide rule:** Handles are never hidden — all 4 remain visible regardless of connection count.
- **Restricted target validation:** Same as LLM — accepts from `ai`, `condition`, `logic`.

#### Orchestrator / Agent Team (`orchestrator`, `agentTeam`)

**Simpler logic than LLM/Agent.** Only needs 1 input, then all others become outputs.

| Connections    | `top`       | `bottom` | `left` | `right` |
| -------------- | ----------- | -------- | ------ | ------- |
| 0 (initial)    | target      | target   | target | target  |
| 1+ (has input) | *connected* | source   | source | source  |

- **Hide rule:** Handles are never hidden.
- **Target validation:** Accepts connections from `ai`, `condition`, `logic`.
- **Source validation:** Can only connect to `ai`, `condition`, `logic` nodes. Cannot connect to `tools`, `output`, `memory`, or `trigger`.

#### Planner (`planner`)

**All handles are TARGET (input).** Planner never outputs — it only receives.

| Handle   | Type   | Direction |
| -------- | ------ | --------- |
| `top`    | target | input     |
| `bottom` | target | input     |
| `left`   | target | input     |
| `right`  | target | input     |

- **Hide rule:** Once any handle is connected, unconnected handles are hidden. Only the connected handle remains.
- **Validation:** Can **only** receive connections from `orchestrator` or `agentTeam` node types. Rejects all other sources.

#### Tool Nodes (`webSearch`, `databaseQuery`, `fileSearch`, `apiCall`, `runCode`)

**All handles are TARGET (input).** Tool nodes never output.

| Handle   | Type   | Direction |
| -------- | ------ | --------- |
| `top`    | target | input     |
| `bottom` | target | input     |
| `left`   | target | input     |
| `right`  | target | input     |

- **Hide rule:** Once any handle is connected, unconnected handles are hidden.
- **Validation:** Can **only** receive connections from `ai` category nodes. Rejects everything else.

#### Condition Node (`condition`)

**Special layout:** Has 3 input handles and 2 labeled output handles.

**Input handles (target):**

| Handle  | Type   | Position     |
| ------- | ------ | ------------ |
| `top`   | target | top center   |
| `left`  | target | left center  |
| `right` | target | right center |

**Output handles (source):**

| Handle  | Type   | Position     | Label          |
| ------- | ------ | ------------ | -------------- |
| `true`  | source | bottom-left  | "true" (green) |
| `false` | source | bottom-right | "false" (red)  |

- The output positions can be **swapped** via the UI (`isSwapped` flips left/right, `isVerticalSwapped` flips top/bottom).
- **Input hide rule:** Once any input handle is connected, unconnected input handles are hidden.
- **Input validation:** Accepts connections from any category **except** `output`.
- **Output validation:** Can connect to any category **except** `trigger`.

> **Note for programmatic edges:** Use `sourceHandle: "true"` or `sourceHandle: "false"` when creating edges from a condition node to specify which branch the edge follows.

#### Memory Node (`memory`)

**All handles are TARGET (input).** Memory never outputs.

| Handle   | Type   | Direction |
| -------- | ------ | --------- |
| `top`    | target | input     |
| `bottom` | target | input     |
| `left`   | target | input     |
| `right`  | target | input     |

- **Hide rule:** Once any handle is connected, unconnected handles are hidden.
- **Validation:** Can **only** receive connections from `ai` category nodes.

#### Human In The Loop (`humanInTheLoop`)

**Handles change type dynamically.** Simpler than LLM/Agent.

| Connections    | `top`       | `bottom` | `left` | `right` |
| -------------- | ----------- | -------- | ------ | ------- |
| 0 (initial)    | target      | target   | target | target  |
| 1+ (has input) | *connected* | source   | source | source  |

- **Hide rule:** When 2+ total connections exist, unconnected handles are hidden.
- **Target validation:** Can only receive from `ai` category nodes.
- **Source validation:** Can connect to any category **except** `trigger`.

#### Parser (`parser`)

**Identical dynamic behavior to LLM Node** — same state machine.

| Connections      | `top`       | `bottom`    | `left`              | `right` |
| ---------------- | ----------- | ----------- | ------------------- | ------- |
| 0 (initial)      | target      | target      | target              | target  |
| 1 input          | *connected* | source      | source              | source  |
| 2 (1 in + 1 out) | *connected* | *connected* | target (restricted) | source  |
| 3+               | balanced    |

- **Hide rule:** Same as LLM — when 3+ connections and 2+ inputs, unconnected handles are hidden.
- **Restricted target validation:** Accepts from `ai`, `condition`, `logic`.
- **Source validation:** Can connect to any category **except** `trigger`.

#### Output Nodes (`chatOutput`, `fileOutput`, `objectOutput`)

**All handles are TARGET (input).** Output nodes are terminal — they never connect onward.

| Handle   | Type   | Direction |
| -------- | ------ | --------- |
| `top`    | target | input     |
| `bottom` | target | input     |
| `left`   | target | input     |
| `right`  | target | input     |

- **Hide rule:** Once any handle is connected, unconnected handles are hidden.
- **Validation:** None — accepts connections from any node category.

#### Summary Table

| Node Type                                                        | Initial Handles                  | Dynamic?     | Max Inputs | Max Outputs | Hide Unconnected        |
| ---------------------------------------------------------------- | -------------------------------- | ------------ | ---------- | ----------- | ----------------------- |
| `chatInput`, `webhook`, `schedule`                               | 3 source (B/L/R)                 | No           | 0          | 3           | After 1st connection    |
| `llmCall`                                                        | 4 target                         | Yes          | Balanced   | Balanced    | After 3 conn + 2 inputs |
| `agent`                                                          | 4 target                         | Yes          | Balanced   | Balanced    | Never                   |
| `orchestrator`, `agentTeam`                                      | 4 target                         | Yes (simple) | 1          | 3           | Never                   |
| `planner`                                                        | 4 target                         | No           | 1          | 0           | After 1st connection    |
| `webSearch`, `databaseQuery`, `fileSearch`, `apiCall`, `runCode` | 4 target                         | No           | 1          | 0           | After 1st connection    |
| `condition`                                                      | 3 target + 2 source (true/false) | No           | 3          | 2           | Inputs: after 1st conn  |
| `memory`                                                         | 4 target                         | No           | 1          | 0           | After 1st connection    |
| `humanInTheLoop`                                                 | 4 target                         | Yes (simple) | 1          | 3           | After 2 connections     |
| `parser`                                                         | 4 target                         | Yes          | Balanced   | Balanced    | After 3 conn + 2 inputs |
| `chatOutput`, `fileOutput`, `objectOutput`                       | 4 target                         | No           | 1          | 0           | After 1st connection    |

#### Practical Guide for Edge Creation

The `create_edge` WebSocket event requires a specific payload structure. **Missing required fields will cause errors.** Here is the exact format:

```json
{
  "environmentId": "env-123",
  "edge": {
    "id": "unique-edge-id-here",
    "source": "source-node-id",
    "target": "target-node-id",
    "type": "default",
    "animated": true,
    "environmentId": "env-123",
    "sourceHandle": "bottom",
    "targetHandle": "top"
  }
}
```

**All required fields:**

| Field           | Required | Description                                                                                                                      |
| --------------- | -------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `id`            | **yes**  | Unique edge ID. Generate a cuid2 or UUID. The backend does `prisma.edge.upsert({ where: { id } })` — without it the query fails. |
| `source`        | **yes**  | ID of the source node (where the edge starts)                                                                                    |
| `target`        | **yes**  | ID of the target node (where the edge ends)                                                                                      |
| `type`          | **yes**  | Always use `"default"`                                                                                                           |
| `animated`      | **yes**  | Use `true` for animated edges                                                                                                    |
| `environmentId` | **yes**  | Must be inside `edge` **and** at the wrapper level. The repository reads it from `edge.environmentId`.                           |
| `sourceHandle`  | no       | Handle ID on source: `"top"`, `"bottom"`, `"left"`, `"right"`, `"true"`, `"false"`                                               |
| `targetHandle`  | no       | Handle ID on target: `"top"`, `"bottom"`, `"left"`, `"right"`                                                                    |

> **Common mistake:** Putting `environmentId` only at the wrapper level. The edge object itself also needs `environmentId` because `edgesService.create(data.edge)` reads it directly from the edge payload.

> **Note about `groupId`:** Don't set it — the backend deletes it (`delete data.edge.groupId`) and auto-resolves group assignment based on the connected nodes.

**Handle tips:**
- `sourceHandle` and `targetHandle` are optional — omit them and the frontend picks the first available.
- For trigger -> ai: `sourceHandle: "bottom"`, `targetHandle: "top"` for vertical flow.
- For condition branches: `sourceHandle: "true"` or `sourceHandle: "false"`.
- For side connections (ai -> tool): `sourceHandle: "left"` or `"right"`, `targetHandle: "right"` or `"left"`.
- The backend does **not** validate handle positions — only node categories. Handles are stored for the frontend to render correctly.

**Python example:**

```python
from cuid2 import cuid_wrapper

cuid = cuid_wrapper()

sio.emit("create_edge", {
    "environmentId": "env-123",
    "edge": {
        "id": cuid(),                    # generate unique ID
        "source": "trigger-node-id",
        "target": "llm-node-id",
        "type": "default",
        "animated": True,
        "environmentId": "env-123",      # must also be here
        "sourceHandle": "bottom",
        "targetHandle": "top",
    },
}, namespace="/canvas")
```

**Rust example:**

```rust
// Add cuid2 = "0.1" to Cargo.toml
let edge_id = cuid2::create_id();

client.emit("create_edge", json!({
    "environmentId": "env-123",
    "edge": {
        "id": edge_id,
        "source": "trigger-node-id",
        "target": "llm-node-id",
        "type": "default",
        "animated": true,
        "environmentId": "env-123",
        "sourceHandle": "bottom",
        "targetHandle": "top"
    }
})).expect("Failed to create edge");
```

---

### Trigger Nodes

Entry points that start a workflow.

#### `chatInput` — Chat Input

Accepts chat messages as input to the workflow.

```json
{
  "type": "chatInput",
  "category": "trigger",
  "data": {
    "label": "Chat Input",
    "config": {
      "variableName": "user_message",
      "inputType": "Text"
    }
  }
}
```

| Field          | Type   | Default          | Description                      |
| -------------- | ------ | ---------------- | -------------------------------- |
| `variableName` | string | `"user_message"` | Variable name to store the input |
| `inputType`    | string | `"Text"`         | Input type                       |

#### `webhook` — Webhook

Receives HTTP POST requests to trigger the workflow.

```json
{
  "type": "webhook",
  "category": "trigger",
  "data": {
    "label": "Webhook",
    "config": {
      "endpointPath": "/webhook",
      "method": "POST",
      "authEnabled": false,
      "authSecret": "",
      "schemaValidation": ""
    }
  }
}
```

| Field              | Type    | Default      | Description                        |
| ------------------ | ------- | ------------ | ---------------------------------- |
| `endpointPath`     | string  | `"/webhook"` | URL path for the webhook           |
| `method`           | string  | `"POST"`     | HTTP method                        |
| `authEnabled`      | boolean | `false`      | Enable authentication              |
| `authSecret`       | string  | `""`         | Secret for authentication          |
| `schemaValidation` | string  | `""`         | JSON schema for payload validation |

#### `schedule` — Schedule

Triggers the workflow on a scheduled interval.

```json
{
  "type": "schedule",
  "category": "trigger",
  "data": {
    "label": "Schedule",
    "config": {
      "cronExpression": "0 0 * * *",
      "timezone": "UTC",
      "repeatNumber": 1,
      "repeatUnit": "days",
      "weekDays": [],
      "startDate": "",
      "endDate": ""
    }
  }
}
```

| Field            | Type     | Default       | Description      |
| ---------------- | -------- | ------------- | ---------------- |
| `cronExpression` | string   | `"0 0 * * *"` | Cron expression  |
| `timezone`       | string   | `"UTC"`       | Timezone         |
| `repeatNumber`   | number   | `1`           | Repeat interval  |
| `repeatUnit`     | string   | `"days"`      | Repeat unit      |
| `weekDays`       | string[] | `[]`          | Days of the week |
| `startDate`      | string   | `""`          | Start date (ISO) |
| `endDate`        | string   | `""`          | End date (ISO)   |

---

### AI Nodes

Intelligent processing nodes powered by LLMs.

#### `llmCall` — LLM Call

Calls a Large Language Model.

```json
{
  "type": "llmCall",
  "category": "ai",
  "data": {
    "label": "LLM Call",
    "config": {
      "model": "gemini-2.5-flash",
      "systemPrompt": "",
      "temperature": 0.5,
      "enableMemory": true
    }
  }
}
```

| Field          | Type    | Default              | Description                |
| -------------- | ------- | -------------------- | -------------------------- |
| `model`        | string  | `"gemini-2.5-flash"` | LLM model to use           |
| `systemPrompt` | string  | `""`                 | System instructions        |
| `temperature`  | number  | `0.5`                | Temperature (0-1)          |
| `enableMemory` | boolean | `true`               | Enable conversation memory |

#### `agent` — Agent

Runs an autonomous agent for multi-step task execution.

```json
{
  "type": "agent",
  "category": "ai",
  "data": {
    "label": "Agent",
    "config": {
      "model": "gemini-1.5-flash",
      "description": "",
      "systemPrompt": "",
      "temperature": 0.5,
      "memoryWindow": "",
      "maxSteps": 10,
      "enableMemory": true
    }
  }
}
```

| Field          | Type    | Default              | Description             |
| -------------- | ------- | -------------------- | ----------------------- |
| `model`        | string  | `"gemini-1.5-flash"` | LLM model               |
| `description`  | string  | `""`                 | Agent description       |
| `systemPrompt` | string  | `""`                 | System instructions     |
| `temperature`  | number  | `0.5`                | Temperature (0-1)       |
| `memoryWindow` | string  | `""`                 | Memory window size      |
| `maxSteps`     | number  | `10`                 | Maximum execution steps |
| `enableMemory` | boolean | `true`               | Enable memory           |

#### `planner` — Planner

Plans and decomposes tasks. Only accepts connections from `orchestrator` or `agentTeam` nodes.

```json
{
  "type": "planner",
  "category": "ai",
  "data": {
    "label": "Planner",
    "config": {
      "model": "gemini-1.5-flash",
      "objectivePrompt": "",
      "allowSuspend": true
    }
  }
}
```

| Field             | Type    | Default              | Description           |
| ----------------- | ------- | -------------------- | --------------------- |
| `model`           | string  | `"gemini-1.5-flash"` | LLM model             |
| `objectivePrompt` | string  | `""`                 | Objective prompt      |
| `allowSuspend`    | boolean | `true`               | Allow task suspension |

#### `orchestrator` / `agentTeam` — Orchestrator / Agent Team

Orchestrates multiple AI agents working together.

```json
{
  "type": "orchestrator",
  "category": "ai",
  "data": {
    "label": "Orchestrator",
    "config": {
      "model": "gemini-1.5-flash",
      "maxPhases": 10,
      "includeExtraInfo": true,
      "enableCritic": false,
      "criticApiKeyId": "",
      "criticModel": "",
      "criticSystemPrompt": "",
      "criticMaxRetries": 3,
      "criticAllowSuspend": true,
      "enablePhaseReactor": false,
      "phaseReactorApiKeyId": "",
      "phaseReactorModel": "",
      "phaseReactorSystemPrompt": "",
      "phaseReactorAllowSuspend": true,
      "enableFinalReactor": false,
      "finalReactorApiKeyId": "",
      "finalReactorModel": "",
      "finalReactorSystemPrompt": "",
      "finalReactorAllowSuspend": true
    }
  }
}
```

| Field                      | Type    | Default              | Description                  |
| -------------------------- | ------- | -------------------- | ---------------------------- |
| `model`                    | string  | `"gemini-1.5-flash"` | LLM model                    |
| `maxPhases`                | number  | `10`                 | Maximum orchestration phases |
| `includeExtraInfo`         | boolean | `true`               | Include extra context info   |
| `enableCritic`             | boolean | `false`              | Enable critic agent          |
| `criticModel`              | string  | `""`                 | Critic LLM model             |
| `criticSystemPrompt`       | string  | `""`                 | Critic system prompt         |
| `criticMaxRetries`         | number  | `3`                  | Critic max retries           |
| `criticAllowSuspend`       | boolean | `true`               | Critic can suspend           |
| `enablePhaseReactor`       | boolean | `false`              | Enable phase reactor         |
| `phaseReactorModel`        | string  | `""`                 | Phase reactor LLM model      |
| `phaseReactorSystemPrompt` | string  | `""`                 | Phase reactor prompt         |
| `phaseReactorAllowSuspend` | boolean | `true`               | Phase reactor can suspend    |
| `enableFinalReactor`       | boolean | `false`              | Enable final reactor         |
| `finalReactorModel`        | string  | `""`                 | Final reactor LLM model      |
| `finalReactorSystemPrompt` | string  | `""`                 | Final reactor prompt         |
| `finalReactorAllowSuspend` | boolean | `true`               | Final reactor can suspend    |

> Use `type: "agentTeam"` for the Agent Team variant — same config and component as `orchestrator`.

---

### Tool Nodes

External integrations and utilities. These are input-only nodes — they cannot output connections.

#### `webSearch` — Web Search

```json
{
  "type": "webSearch",
  "category": "tools",
  "data": {
    "label": "Web Search",
    "config": {
      "query": "",
      "provider": "google",
      "maxResults": 5
    }
  }
}
```

| Field        | Type   | Default    | Description           |
| ------------ | ------ | ---------- | --------------------- |
| `query`      | string | `""`       | Search query          |
| `provider`   | string | `"google"` | Search provider       |
| `maxResults` | number | `5`        | Max results to return |

#### `databaseQuery` — Database Query

```json
{
  "type": "databaseQuery",
  "category": "tools",
  "data": {
    "label": "Database Query",
    "config": {
      "connectionId": "",
      "queryType": "sql",
      "query": "",
      "parameters": ""
    }
  }
}
```

| Field          | Type   | Default | Description            |
| -------------- | ------ | ------- | ---------------------- |
| `connectionId` | string | `""`    | Database connection ID |
| `queryType`    | string | `"sql"` | Query type             |
| `query`        | string | `""`    | SQL query              |
| `parameters`   | string | `""`    | Query parameters       |

#### `fileSearch` — File Search

```json
{
  "type": "fileSearch",
  "category": "tools",
  "data": {
    "label": "File Search",
    "config": {
      "knowledgeBaseId": "",
      "strategy": "semantic",
      "searchQuery": "",
      "topK": 5,
      "minScore": 0.7
    }
  }
}
```

| Field             | Type   | Default      | Description             |
| ----------------- | ------ | ------------ | ----------------------- |
| `knowledgeBaseId` | string | `""`         | Knowledge base ID       |
| `strategy`        | string | `"semantic"` | Search strategy         |
| `searchQuery`     | string | `""`         | Search query            |
| `topK`            | number | `5`          | Number of top results   |
| `minScore`        | number | `0.7`        | Minimum relevance score |

#### `apiCall` — API Call

```json
{
  "type": "apiCall",
  "category": "tools",
  "data": {
    "label": "API Call",
    "config": {
      "url": "",
      "endpoint": "",
      "method": "GET",
      "headers": "",
      "body": "",
      "queryParameters": "",
      "timeout": 5000,
      "description": ""
    }
  }
}
```

| Field             | Type   | Default | Description                   |
| ----------------- | ------ | ------- | ----------------------------- |
| `url`             | string | `""`    | Base URL                      |
| `endpoint`        | string | `""`    | API endpoint path             |
| `method`          | string | `"GET"` | HTTP method                   |
| `headers`         | string | `""`    | Request headers (JSON string) |
| `body`            | string | `""`    | Request body                  |
| `queryParameters` | string | `""`    | Query parameters              |
| `timeout`         | number | `5000`  | Timeout in milliseconds       |
| `description`     | string | `""`    | Description of this API call  |

#### `runCode` — Run Code

```json
{
  "type": "runCode",
  "category": "tools",
  "data": {
    "label": "Run Code",
    "config": {
      "runtime": "python3.10",
      "code": "",
      "inputVariables": "",
      "outputVariables": ""
    }
  }
}
```

| Field             | Type   | Default        | Description              |
| ----------------- | ------ | -------------- | ------------------------ |
| `runtime`         | string | `"python3.10"` | Code runtime environment |
| `code`            | string | `""`           | Code to execute          |
| `inputVariables`  | string | `""`           | Input variable names     |
| `outputVariables` | string | `""`           | Output variable names    |

---

### Logic Nodes

Flow control and data processing.

#### `condition` — Condition

Branches the workflow based on conditions. Has `true` and `false` output paths.

```json
{
  "type": "condition",
  "category": "condition",
  "data": {
    "label": "Condition",
    "config": {
      "conditionType": "basic",
      "model": "gemini-1.5-flash",
      "evaluationPrompt": "",
      "contextVariables": [],
      "variable": "",
      "operator": "equals",
      "value": ""
    }
  }
}
```

| Field              | Type   | Default              | Description                          |
| ------------------ | ------ | -------------------- | ------------------------------------ |
| `conditionType`    | string | `"basic"`            | Condition type (`"basic"` or `"ai"`) |
| `model`            | string | `"gemini-1.5-flash"` | LLM model (for AI conditions)        |
| `evaluationPrompt` | string | `""`                 | Prompt for AI evaluation             |
| `contextVariables` | array  | `[]`                 | Context variables                    |
| `variable`         | string | `""`                 | Variable to evaluate (basic mode)    |
| `operator`         | string | `"equals"`           | Comparison operator                  |
| `value`            | string | `""`                 | Value to compare against             |

> **Note:** The `condition` node uses `category: "condition"` (not `"logic"`).

#### `memory` — Memory

Stores and retrieves conversation memory. Input-only node.

```json
{
  "type": "memory",
  "category": "logic",
  "data": {
    "label": "Memory",
    "config": {
      "memoryType": "longTerm",
      "action": "retrieve",
      "retrievalStrategy": "similarity",
      "key": "",
      "content": ""
    }
  }
}
```

| Field               | Type   | Default        | Description                        |
| ------------------- | ------ | -------------- | ---------------------------------- |
| `memoryType`        | string | `"longTerm"`   | Memory type                        |
| `action`            | string | `"retrieve"`   | Action (`"retrieve"` or `"store"`) |
| `retrievalStrategy` | string | `"similarity"` | Retrieval strategy                 |
| `key`               | string | `""`           | Memory key                         |
| `content`           | string | `""`           | Content to store                   |

#### `humanInTheLoop` — Human In The Loop

Pauses the workflow for human review or approval.

```json
{
  "type": "humanInTheLoop",
  "category": "logic",
  "data": {
    "label": "Human In The Loop",
    "config": {
      "interactionType": "approval",
      "timeout": 24,
      "onTimeout": "approve"
    }
  }
}
```

| Field             | Type   | Default      | Description                                   |
| ----------------- | ------ | ------------ | --------------------------------------------- |
| `interactionType` | string | `"approval"` | Interaction type                              |
| `timeout`         | number | `24`         | Timeout in hours                              |
| `onTimeout`       | string | `"approve"`  | Action on timeout (`"approve"` or `"reject"`) |

#### `parser` — Parser

Parses and transforms data using an LLM.

```json
{
  "type": "parser",
  "category": "logic",
  "data": {
    "label": "Parser",
    "config": {
      "model": "gemini-1.5-flash",
      "prompt": "",
      "schema": "",
      "temperature": 0.5
    }
  }
}
```

| Field         | Type   | Default              | Description                          |
| ------------- | ------ | -------------------- | ------------------------------------ |
| `model`       | string | `"gemini-1.5-flash"` | LLM model                            |
| `prompt`      | string | `""`                 | Parsing instructions                 |
| `schema`      | string | `""`                 | Expected output schema (JSON string) |
| `temperature` | number | `0.5`                | Temperature (0-1)                    |

---

### Output Nodes

Terminal nodes that produce the final workflow output. Input-only — they cannot connect to other nodes.

#### `chatOutput` — Chat Output

Returns the response as a chat message.

```json
{
  "type": "chatOutput",
  "category": "output",
  "data": {
    "label": "Chat Output",
    "config": {
      "apiKeyId": "",
      "model": "",
      "messageType": ["text"],
      "instructions": "",
      "enableSummary": true,
      "outputVariables": []
    }
  }
}
```

| Field             | Type     | Default    | Description                            |
| ----------------- | -------- | ---------- | -------------------------------------- |
| `apiKeyId`        | string   | `""`       | API key ID for the LLM                 |
| `model`           | string   | `""`       | LLM model for response generation      |
| `messageType`     | string[] | `["text"]` | Output message types                   |
| `instructions`    | string   | `""`       | Instructions for generating the output |
| `enableSummary`   | boolean  | `true`     | Enable summary generation              |
| `outputVariables` | array    | `[]`       | Output variables to include            |

#### `fileOutput` — File Output

Exports the result to a file.

```json
{
  "type": "fileOutput",
  "category": "output",
  "data": {
    "label": "File Output",
    "config": {
      "fileName": "user_output",
      "mimeType": "text/plain",
      "action": "download",
      "content": ""
    }
  }
}
```

| Field      | Type   | Default         | Description           |
| ---------- | ------ | --------------- | --------------------- |
| `fileName` | string | `"user_output"` | Output file name      |
| `mimeType` | string | `"text/plain"`  | MIME type             |
| `action`   | string | `"download"`    | Action on completion  |
| `content`  | string | `""`            | File content template |

#### `objectOutput` — Object Output

Outputs a structured JSON object.

```json
{
  "type": "objectOutput",
  "category": "output",
  "data": {
    "label": "Object Output",
    "config": {
      "model": "gemini-1.5-flash",
      "prompt": "",
      "schema": "",
      "temperature": 0.5
    }
  }
}
```

| Field         | Type   | Default              | Description                 |
| ------------- | ------ | -------------------- | --------------------------- |
| `model`       | string | `"gemini-1.5-flash"` | LLM model                   |
| `prompt`      | string | `""`                 | Generation prompt           |
| `schema`      | string | `""`                 | Expected output JSON schema |
| `temperature` | number | `0.5`                | Temperature (0-1)           |

---

### Quick Example: Create a Complete Workflow via WebSocket

This example creates a `chatInput` -> `llmCall` -> `chatOutput` workflow:

```python
import socketio
import requests
import time

API_URL = "http://localhost:8080"
ENV_ID = "your-environment-id"

# 1. Login
res = requests.post(f"{API_URL}/api/auth/login", json={
    "email": "user@example.com",
    "password": "your_password",
})
token = res.cookies.get("better-auth.session_token")
cookie = f"better-auth.session_token={token}"

# 2. Connect to canvas
sio = socketio.Client()
created_nodes = []

@sio.on("node_created", namespace="/canvas")
def on_node(node):
    created_nodes.append(node)
    print(f"Node created: {node['id']} (type={node['type']})")

@sio.on("edge_created", namespace="/canvas")
def on_edge(edge):
    print(f"Edge created: {edge['source']} -> {edge['target']}")

@sio.on("group_created", namespace="/canvas")
def on_group(group):
    print(f"Group auto-created: {group['id']}")

sio.connect(API_URL, namespaces=["/canvas"], headers={"Cookie": cookie})

# 3. Load canvas state (joins the environment room)
sio.emit("load_canvas_state", {"environmentId": ENV_ID}, namespace="/canvas")
time.sleep(1)

# 4. Create trigger node
sio.emit("create_node", {
    "environmentId": ENV_ID,
    "node": {
        "type": "chatInput",
        "category": "trigger",
        "position": {"x": 100, "y": 100},
        "data": {
            "label": "Chat Input",
            "config": {"variableName": "user_message", "inputType": "Text"},
        },
    },
}, namespace="/canvas")
time.sleep(1)

# 5. Create AI node
sio.emit("create_node", {
    "environmentId": ENV_ID,
    "node": {
        "type": "llmCall",
        "category": "ai",
        "position": {"x": 100, "y": 300},
        "data": {
            "label": "LLM Call",
            "config": {"model": "gemini-2.5-flash", "systemPrompt": "You are a helpful assistant.", "temperature": 0.7, "enableMemory": True},
        },
    },
}, namespace="/canvas")
time.sleep(1)

# 6. Create output node
sio.emit("create_node", {
    "environmentId": ENV_ID,
    "node": {
        "type": "chatOutput",
        "category": "output",
        "position": {"x": 100, "y": 500},
        "data": {
            "label": "Chat Output",
            "config": {"messageType": ["text"], "enableSummary": True, "outputVariables": []},
        },
    },
}, namespace="/canvas")
time.sleep(1)

# 7. Connect nodes with edges (trigger -> ai -> output)
from cuid2 import cuid_wrapper
cuid = cuid_wrapper()

if len(created_nodes) >= 3:
    sio.emit("create_edge", {
        "environmentId": ENV_ID,
        "edge": {
            "id": cuid(),
            "source": created_nodes[0]["id"],
            "target": created_nodes[1]["id"],
            "type": "default",
            "animated": True,
            "environmentId": ENV_ID,
            "sourceHandle": "bottom",
            "targetHandle": "top",
        },
    }, namespace="/canvas")
    time.sleep(1)

    sio.emit("create_edge", {
        "environmentId": ENV_ID,
        "edge": {
            "id": cuid(),
            "source": created_nodes[1]["id"],
            "target": created_nodes[2]["id"],
            "type": "default",
            "animated": True,
            "environmentId": ENV_ID,
            "sourceHandle": "bottom",
            "targetHandle": "top",
        },
    }, namespace="/canvas")

sio.wait()
```

---

## WebSocket Namespaces

### Canvas (`/canvas`)

Real-time collaboration on the visual canvas (nodes, edges, groups). This is the primary namespace for the agent builder.

**Source:** `apps/api/src/events/events.gateway.ts`

#### Rooms

Clients join a room per environment:

- Pattern: `environment:{environmentId}`
- Joined automatically when `load_canvas_state` is called.
- Also supports legacy `join_room` / `leave_room` with arbitrary room IDs.

#### Events: Client -> Server

| Event                   | Payload                                                                 | Description                                                                     | Returns                                   |
| ----------------------- | ----------------------------------------------------------------------- | ------------------------------------------------------------------------------- | ----------------------------------------- |
| `join_room`             | `roomId: string`                                                        | Join a room manually                                                            | `{ event: 'joined_room', room }`          |
| `leave_room`            | `roomId: string`                                                        | Leave a room                                                                    | `{ event: 'left_room', room }`            |
| `cursor_move`           | `{ roomId: string, x: number, y: number, userId: string }`              | Broadcast cursor position (volatile, may be dropped)                            | —                                         |
| `load_canvas_state`     | `{ environmentId: string }`                                             | Load full canvas state (nodes, edges, groups). Auto-joins the environment room. | Emits `canvas_state_loaded` to the caller |
| `create_node`           | `{ environmentId: string, node: CreateNodePayload }`                    | Create a new node                                                               | Created node object                       |
| `update_node`           | `{ environmentId: string, id: string, updates: object }`                | Update a node (position, data, groupId, etc.)                                   | Updated node object                       |
| `delete_node`           | `{ environmentId: string, nodeId: string }`                             | Delete a node and resolve groups                                                | `{ success: true }`                       |
| `node_update`           | `{ roomId: string, nodeId: string, changes: any }`                      | Broadcast node changes to a room (optimistic)                                   | —                                         |
| `create_edge`           | `{ environmentId: string, edge: CreateEdgePayload }`                    | Create a new edge. Triggers automatic group resolution.                         | Created edge object                       |
| `update_edge`           | `{ environmentId: string, id: string, updates: object }`                | Update an edge                                                                  | Updated edge object                       |
| `delete_edge`           | `{ environmentId: string, edgeId: string }`                             | Delete an edge and resolve groups                                               | `{ success: true }`                       |
| `edge_update`           | `{ roomId: string, edgeId: string, changes: any }`                      | Broadcast edge changes to a room (optimistic)                                   | —                                         |
| `create_group`          | `{ environmentId: string, id?: string, label?: string, name?: string }` | Create a group                                                                  | Created group object                      |
| `update_group`          | `{ environmentId: string, id: string, name?: string, label?: string }`  | Update a group name/label                                                       | Updated group object                      |
| `delete_group`          | `{ environmentId: string, id: string }`                                 | Delete a group                                                                  | `{ success: true }`                       |
| `update_elements_order` | `{ environmentId: string, updates: ElementOrderUpdate[] }`              | Reorder elements on the canvas                                                  | `{ success: true }`                       |
| `get_groups`            | `{ environmentId: string }`                                             | Get groups for an environment                                                   | —                                         |

**`CreateNodePayload`:**

```typescript
{
  type: string;                           // e.g. 'triggerNode'
  category?: string;                      // e.g. 'trigger' (defaults to 'default')
  position: { x: number; y: number };
  data?: any;                             // node-specific data
  measured?: { width: number; height: number };
  groupId?: string | null;
  sortIndex?: number;
}
```

**`CreateEdgePayload`:**

```typescript
{
  id: string;               // REQUIRED — unique edge ID (cuid2). The backend upserts by this ID.
  source: string;           // REQUIRED — source node ID
  target: string;           // REQUIRED — target node ID
  type: string;             // REQUIRED — edge type, use "default"
  animated: boolean;        // REQUIRED — animate the edge line, use true
  environmentId: string;    // REQUIRED — must match the wrapper environmentId
  sourceHandle?: string;    // Optional — handle ID on source node ("top", "bottom", "left", "right", "true", "false")
  targetHandle?: string;    // Optional — handle ID on target node
}
```

> **Important:** The `id` field is mandatory — the backend does an upsert (`prisma.edge.upsert`) using `where: { id }`. If you omit it, the query fails. Generate a unique ID (cuid2 recommended, any unique string works).

**`ElementOrderUpdate`:**

```typescript
{
  id: string;
  type: 'node' | 'group';
  sortIndex: number;
}
```

#### Events: Server -> Client

These are broadcast to all clients in the `environment:{environmentId}` room.

| Event                    | Payload                                                            | Description                                        |
| ------------------------ | ------------------------------------------------------------------ | -------------------------------------------------- |
| `canvas_state_loaded`    | `{ environmentId, nodes: Node[], edges: Edge[], groups: Group[] }` | Full canvas state (sent only to requesting client) |
| `node_created`           | `Node`                                                             | A new node was created                             |
| `node_updated`           | `Node`                                                             | A node was updated                                 |
| `node_deleted`           | `nodeId: string`                                                   | A node was deleted                                 |
| `edge_created`           | `Edge`                                                             | A new edge was created                             |
| `edge_updated`           | `Edge`                                                             | An edge was updated                                |
| `edge_deleted`           | `edgeId: string`                                                   | An edge was deleted                                |
| `group_created`          | `Group`                                                            | A new group was created                            |
| `group_updated`          | `Group`                                                            | A group was updated                                |
| `group_deleted`          | `groupId: string`                                                  | A group was deleted                                |
| `elements_order_updated` | `ElementOrderUpdate[]`                                             | Element sort order changed                         |
| `cursor_moved`           | `{ roomId, x, y, userId }`                                         | Another user's cursor moved (volatile)             |

#### Group Auto-Resolution

The canvas gateway automatically manages group lifecycle when edges are created or deleted:

- **Edge created between two ungrouped nodes:** A new group is created and both nodes are assigned.
- **Edge created between a grouped and an ungrouped node:** The ungrouped node joins the existing group.
- **Edge created between two nodes in different groups:** Groups are merged (the larger group survives).
- **Edge/node deleted:** If the group splits into disconnected components, new groups are created for smaller components. If no edges remain, the group is deleted.

---

### Project (`/project`)

Project-level events for managing rooms and environment positions.

**Source:** `apps/api/src/events/project.gateway.ts`

#### Rooms

- Pattern: `project_{projectId}`

#### Events: Client -> Server

| Event                          | Payload                                                              | Description                           | Returns                                             |
| ------------------------------ | -------------------------------------------------------------------- | ------------------------------------- | --------------------------------------------------- |
| `join_project`                 | `projectId: string`                                                  | Join the project room                 | `{ event: 'joined_project', room }`                 |
| `leave_project`                | `projectId: string`                                                  | Leave the project room                | `{ event: 'left_project', room }`                   |
| `update_environment_positions` | `{ projectId: string, updates: { id: string, position: number }[] }` | Reorder environments within a project | `{ event: 'update_environment_positions_success' }` |

#### Events: Server -> Client

| Event                    | Payload                              | Description                        |
| ------------------------ | ------------------------------------ | ---------------------------------- |
| `joined_project`         | `{ room: string }`                   | Confirmation of room join          |
| `environments_reordered` | `{ id: string, position: number }[]` | Environment positions were updated |

---

### Speech (`/speech`)

Real-time speech-to-text transcription using Google Cloud Speech-to-Text API.

**Source:** `apps/api/src/chat/presentation/speech.gateway.ts`

> **Note:** This namespace creates a fresh connection per recording session (`forceNew: true` on the client). The stream is automatically cleaned up on disconnect.

#### Events: Client -> Server

| Event             | Payload                                                                             | Description                                                                      | Returns                          |
| ----------------- | ----------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- | -------------------------------- |
| `start_recording` | `{ sampleRateHertz?: number, languageCode?: string, mimeType?: string }` (optional) | Start a new speech recognition stream. Defaults: 48000 Hz, `es-ES`, `WEBM_OPUS`. | `{ event: 'recording_started' }` |
| `audio_data`      | `Buffer \| ArrayBuffer \| Base64 string`                                            | Send an audio chunk. Emitted every ~250ms from the client's MediaRecorder.       | —                                |
| `stop_recording`  | _(none)_                                                                            | Stop the recognition stream                                                      | `{ event: 'recording_stopped' }` |

**Supported audio buffer formats:**

- `Buffer`
- `ArrayBuffer`
- Base64-encoded string
- JSON Buffer object: `{ type: 'Buffer', data: number[] }`

#### Events: Server -> Client

| Event           | Payload                                    | Description                                                                  |
| --------------- | ------------------------------------------ | ---------------------------------------------------------------------------- |
| `speech_result` | `{ transcript: string, isFinal: boolean }` | Transcription result. `isFinal=false` for interim results, `true` for final. |
| `speech_error`  | `{ message: string }`                      | Error during speech recognition                                              |

---

## Infrastructure

### Redis Adapter

The server uses `@socket.io/redis-adapter` to broadcast events across multiple server instances (horizontal scaling).

**Source:** `apps/api/src/events/redis-io.adapter.ts`

**Setup in `main.ts`:**

```typescript
const redisIoAdapter = new RedisIoAdapter(app, configService);
await redisIoAdapter.connectToRedis();
app.useWebSocketAdapter(redisIoAdapter);
```

The adapter creates a pub/sub Redis client pair. All Socket.IO events are published to Redis and received by every server instance, ensuring clients connected to different instances receive the same broadcasts.

### Access Validation

Canvas events that modify data validate that the authenticated user has access to the target environment:

1. Extracts `userId` from the socket (set during auth).
2. Looks up the environment and its parent project.
3. Checks if the user belongs to the project's workspace.
4. Throws `WsException('Forbidden')` if access is denied.

---

## Environment Variables

| Variable              | Required   | Default      | Description                              |
| --------------------- | ---------- | ------------ | ---------------------------------------- |
| `PORT`                | No         | `8080`       | Server port (HTTP + WebSocket)           |
| `REDIS_HOST`          | No         | `localhost`  | Redis host for Socket.IO adapter         |
| `REDIS_PORT`          | No         | `6379`       | Redis port                               |
| `ALLOWED_ORIGINS`     | No         | `true` (all) | Comma-separated allowed CORS origins     |
| `GOOGLE_CLIENT_EMAIL` | For speech | —            | Google Cloud service account email       |
| `GOOGLE_PRIVATE_KEY`  | For speech | —            | Google Cloud service account private key |
| `GOOGLE_PROJECT_ID`   | For speech | —            | Google Cloud project ID                  |

**Client-side:**

| Variable              | Default                 | Description                            |
| --------------------- | ----------------------- | -------------------------------------- |
| `NEXT_PUBLIC_API_URL` | `http://localhost:8080` | Base URL for all WebSocket connections |

---

## Client Example

Full example connecting to all three namespaces:

```typescript
import { io, Socket } from 'socket.io-client';

const API_URL = process.env.NEXT_PUBLIC_API_URL || 'http://localhost:8080';

// --- Canvas namespace ---
const canvasSocket = io(`${API_URL}/canvas`, {
  withCredentials: true,
  transports: ['websocket', 'polling'],
});

// Load canvas state for an environment
canvasSocket.emit('load_canvas_state', { environmentId: 'env-123' });

canvasSocket.on('canvas_state_loaded', (data) => {
  console.log('Nodes:', data.nodes);
  console.log('Edges:', data.edges);
  console.log('Groups:', data.groups);
});

// Listen for real-time updates
canvasSocket.on('node_created', (node) => { /* handle */ });
canvasSocket.on('node_updated', (node) => { /* handle */ });
canvasSocket.on('node_deleted', (nodeId) => { /* handle */ });
canvasSocket.on('edge_created', (edge) => { /* handle */ });
canvasSocket.on('edge_updated', (edge) => { /* handle */ });
canvasSocket.on('edge_deleted', (edgeId) => { /* handle */ });
canvasSocket.on('group_created', (group) => { /* handle */ });
canvasSocket.on('group_updated', (group) => { /* handle */ });
canvasSocket.on('group_deleted', (groupId) => { /* handle */ });
canvasSocket.on('elements_order_updated', (updates) => { /* handle */ });

// Create a node
canvasSocket.emit('create_node', {
  environmentId: 'env-123',
  node: {
    type: 'triggerNode',
    category: 'trigger',
    position: { x: 100, y: 200 },
    data: { label: 'My Node' },
  },
});

// --- Project namespace ---
const projectSocket = io(`${API_URL}/project`, {
  withCredentials: true,
  transports: ['websocket', 'polling'],
});

projectSocket.emit('join_project', 'project-456');
projectSocket.on('joined_project', (data) => console.log('Joined:', data.room));
projectSocket.on('environments_reordered', (updates) => { /* handle */ });

// --- Speech namespace (on-demand) ---
const speechSocket = io(`${API_URL}/speech`, {
  withCredentials: true,
  transports: ['websocket', 'polling'],
  forceNew: true, // fresh connection per recording session
});

speechSocket.emit('start_recording', {
  sampleRateHertz: 48000,
  languageCode: 'es-ES',
});

speechSocket.on('speech_result', ({ transcript, isFinal }) => {
  console.log(`${isFinal ? '[FINAL]' : '[INTERIM]'} ${transcript}`);
});

speechSocket.on('speech_error', ({ message }) => {
  console.error('Speech error:', message);
});

// Send audio chunks from MediaRecorder
// mediaRecorder.ondataavailable = (e) => speechSocket.emit('audio_data', e.data);

speechSocket.emit('stop_recording');
```

---

## Client Example: Python

Requires [`python-socketio`](https://pypi.org/project/python-socketio/) and `requests`:

```bash
pip install "python-socketio[client]" requests
```

### Authenticate — Obtain session token

```python
import requests

API_URL = "http://localhost:8080"

# Login to get session token
response = requests.post(f"{API_URL}/api/auth/login", json={
    "email": "user@example.com",
    "password": "your_password",
})

# Extract the session token from cookies
session_token = response.cookies.get("better-auth.session_token")
if not session_token:
    raise Exception(f"Login failed: {response.status_code} {response.text}")

SESSION_COOKIE = f"better-auth.session_token={session_token}"
print(f"Authenticated. Token: {session_token[:20]}...")
```

### Canvas — Load state and listen for updates

```python
import socketio

API_URL = "http://localhost:8080"
SESSION_COOKIE = "better-auth.session_token=<your-session-token>"  # or use the token from the login step above

# --- Canvas namespace ---
canvas = socketio.Client()

@canvas.event(namespace="/canvas")
def connect():
    print("Connected to /canvas")
    canvas.emit("load_canvas_state", {"environmentId": "env-123"}, namespace="/canvas")

@canvas.on("canvas_state_loaded", namespace="/canvas")
def on_canvas_state(data):
    print(f"Nodes: {len(data['nodes'])}, Edges: {len(data['edges'])}, Groups: {len(data['groups'])}")

@canvas.on("node_created", namespace="/canvas")
def on_node_created(node):
    print(f"Node created: {node['id']} (type={node['type']})")

@canvas.on("node_updated", namespace="/canvas")
def on_node_updated(node):
    print(f"Node updated: {node['id']}")

@canvas.on("node_deleted", namespace="/canvas")
def on_node_deleted(node_id):
    print(f"Node deleted: {node_id}")

@canvas.on("edge_created", namespace="/canvas")
def on_edge_created(edge):
    print(f"Edge created: {edge['id']} ({edge['source']} -> {edge['target']})")

@canvas.on("edge_deleted", namespace="/canvas")
def on_edge_deleted(edge_id):
    print(f"Edge deleted: {edge_id}")

@canvas.on("group_created", namespace="/canvas")
def on_group_created(group):
    print(f"Group created: {group['id']}")

@canvas.on("group_deleted", namespace="/canvas")
def on_group_deleted(group_id):
    print(f"Group deleted: {group_id}")

canvas.connect(
    API_URL,
    namespaces=["/canvas"],
    headers={"Cookie": SESSION_COOKIE},
    transports=["websocket", "polling"],
)

canvas.wait()
```

### Canvas — Create nodes and edges

```python
import socketio

API_URL = "http://localhost:8080"
SESSION_COOKIE = "better-auth.session_token=<your-session-token>"
ENV_ID = "env-123"

canvas = socketio.Client()

@canvas.event(namespace="/canvas")
def connect():
    print("Connected — creating node...")
    canvas.emit("create_node", {
        "environmentId": ENV_ID,
        "node": {
            "type": "triggerNode",
            "category": "trigger",
            "position": {"x": 100, "y": 200},
            "data": {"label": "My Python Node"},
        },
    }, namespace="/canvas")

@canvas.on("node_created", namespace="/canvas")
def on_node_created(node):
    print(f"Node created: {node['id']}")

    # Update the node
    canvas.emit("update_node", {
        "environmentId": ENV_ID,
        "id": node["id"],
        "updates": {"position": {"x": 300, "y": 400}},
    }, namespace="/canvas")

    # Delete the node
    # canvas.emit("delete_node", {"environmentId": ENV_ID, "nodeId": node["id"]}, namespace="/canvas")

canvas.connect(API_URL, namespaces=["/canvas"], headers={"Cookie": SESSION_COOKIE})
canvas.wait()
```

### Project — Join room and reorder environments

```python
import socketio

API_URL = "http://localhost:8080"
SESSION_COOKIE = "better-auth.session_token=<your-session-token>"

project = socketio.Client()

@project.event(namespace="/project")
def connect():
    print("Connected to /project")
    project.emit("join_project", "project-456", namespace="/project")

@project.on("joined_project", namespace="/project")
def on_joined(data):
    print(f"Joined room: {data['room']}")

    # Reorder environments
    project.emit("update_environment_positions", {
        "projectId": "project-456",
        "updates": [
            {"id": "env-1", "position": 0},
            {"id": "env-2", "position": 1},
            {"id": "env-3", "position": 2},
        ],
    }, namespace="/project")

@project.on("environments_reordered", namespace="/project")
def on_reordered(updates):
    print(f"Environments reordered: {updates}")

project.connect(API_URL, namespaces=["/project"], headers={"Cookie": SESSION_COOKIE})
project.wait()
```

### Speech — Real-time audio transcription

```python
import socketio
import base64

API_URL = "http://localhost:8080"
SESSION_COOKIE = "better-auth.session_token=<your-session-token>"

speech = socketio.Client()

@speech.event(namespace="/speech")
def connect():
    print("Connected to /speech — starting recording...")
    speech.emit("start_recording", {
        "sampleRateHertz": 48000,
        "languageCode": "es-ES",
    }, namespace="/speech")

@speech.on("speech_result", namespace="/speech")
def on_result(data):
    prefix = "[FINAL]" if data["isFinal"] else "[INTERIM]"
    print(f"{prefix} {data['transcript']}")

@speech.on("speech_error", namespace="/speech")
def on_error(data):
    print(f"Error: {data['message']}")

speech.connect(API_URL, namespaces=["/speech"], headers={"Cookie": SESSION_COOKIE})

# Send audio chunks (e.g. from a file)
with open("audio.webm", "rb") as f:
    while chunk := f.read(4096):
        speech.emit("audio_data", base64.b64encode(chunk).decode(), namespace="/speech")

speech.emit("stop_recording", namespace="/speech")
speech.wait()
```

---

## Client Example: Rust

Requires [`rust_socketio`](https://crates.io/crates/rust_socketio) and [`reqwest`](https://crates.io/crates/reqwest):

```toml
# Cargo.toml
[dependencies]
rust_socketio = "0.7"
serde_json = "1"
reqwest = { version = "0.12", features = ["blocking", "cookies", "json"] }
```

### Authenticate — Obtain session token

```rust
use reqwest::blocking::Client;
use reqwest::cookie::Jar;
use std::sync::Arc;

fn get_session_token(api_url: &str, email: &str, password: &str) -> String {
    let jar = Arc::new(Jar::default());
    let client = Client::builder()
        .cookie_provider(jar.clone())
        .build()
        .expect("Failed to build HTTP client");

    let res = client
        .post(format!("{api_url}/api/auth/login"))
        .json(&serde_json::json!({
            "email": email,
            "password": password,
        }))
        .send()
        .expect("Login request failed");

    if !res.status().is_success() {
        panic!("Login failed: {} {}", res.status(), res.text().unwrap_or_default());
    }

    // Extract session token from Set-Cookie header
    let url = api_url.parse().unwrap();
    let cookies = jar.cookies(&url).expect("No cookies returned");
    let cookie_str = cookies.to_str().unwrap();

    cookie_str
        .split("; ")
        .find(|c| c.starts_with("better-auth.session_token="))
        .map(|c| c.trim_start_matches("better-auth.session_token=").to_string())
        .expect("Session token not found in response cookies")
}

fn main() {
    let api_url = "http://localhost:8080";
    let token = get_session_token(api_url, "user@example.com", "your_password");
    let session_cookie = format!("better-auth.session_token={token}");
    println!("Authenticated. Token: {}...", &token[..20]);

    // Use `session_cookie` in all WebSocket connections below
}
```

### Canvas — Load state and listen for updates

```rust
use rust_socketio::{ClientBuilder, Payload, RawClient};
use serde_json::json;
use std::time::Duration;

fn main() {
    let api_url = "http://localhost:8080";
    let session_cookie = "better-auth.session_token=<your-session-token>";

    let canvas = ClientBuilder::new(api_url)
        .namespace("/canvas")
        .opening_header("Cookie", session_cookie)
        .transport_type(rust_socketio::TransportType::Websocket)
        .on("connect", |_payload, client, _| {
            println!("Connected to /canvas");
            client
                .emit(
                    "load_canvas_state",
                    json!({"environmentId": "env-123"}),
                )
                .expect("Failed to emit load_canvas_state");
        })
        .on("canvas_state_loaded", |payload, _client, _| {
            if let Payload::Text(values) = payload {
                println!("Canvas state loaded: {}", values[0]);
            }
        })
        .on("node_created", |payload, _client, _| {
            if let Payload::Text(values) = payload {
                println!("Node created: {}", values[0]);
            }
        })
        .on("node_updated", |payload, _client, _| {
            if let Payload::Text(values) = payload {
                println!("Node updated: {}", values[0]);
            }
        })
        .on("node_deleted", |payload, _client, _| {
            if let Payload::Text(values) = payload {
                println!("Node deleted: {}", values[0]);
            }
        })
        .on("edge_created", |payload, _client, _| {
            if let Payload::Text(values) = payload {
                println!("Edge created: {}", values[0]);
            }
        })
        .on("edge_deleted", |payload, _client, _| {
            if let Payload::Text(values) = payload {
                println!("Edge deleted: {}", values[0]);
            }
        })
        .on("group_created", |payload, _client, _| {
            if let Payload::Text(values) = payload {
                println!("Group created: {}", values[0]);
            }
        })
        .on("group_deleted", |payload, _client, _| {
            if let Payload::Text(values) = payload {
                println!("Group deleted: {}", values[0]);
            }
        })
        .on("elements_order_updated", |payload, _client, _| {
            if let Payload::Text(values) = payload {
                println!("Elements reordered: {}", values[0]);
            }
        })
        .connect()
        .expect("Failed to connect to /canvas");

    // Keep the client alive
    std::thread::sleep(Duration::from_secs(300));
}
```

### Canvas — Create, update, and delete nodes

```rust
use rust_socketio::{ClientBuilder, Payload, RawClient};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn main() {
    let api_url = "http://localhost:8080";
    let session_cookie = "better-auth.session_token=<your-session-token>";
    let env_id = "env-123";

    let env_id_clone = env_id.to_string();

    let canvas = ClientBuilder::new(api_url)
        .namespace("/canvas")
        .opening_header("Cookie", session_cookie)
        .transport_type(rust_socketio::TransportType::Websocket)
        .on("connect", move |_payload, client, _| {
            println!("Connected — creating node...");
            client
                .emit(
                    "create_node",
                    json!({
                        "environmentId": env_id_clone,
                        "node": {
                            "type": "triggerNode",
                            "category": "trigger",
                            "position": {"x": 100, "y": 200},
                            "data": {"label": "My Rust Node"}
                        }
                    }),
                )
                .expect("Failed to emit create_node");
        })
        .on("node_created", |payload, client, _| {
            if let Payload::Text(values) = payload {
                let node = &values[0];
                let node_id = node["id"].as_str().unwrap_or("unknown");
                println!("Node created: {node_id}");

                // Update the node position
                client
                    .emit(
                        "update_node",
                        json!({
                            "environmentId": "env-123",
                            "id": node_id,
                            "updates": {"position": {"x": 300, "y": 400}}
                        }),
                    )
                    .expect("Failed to emit update_node");
            }
        })
        .on("node_updated", |payload, _client, _| {
            if let Payload::Text(values) = payload {
                println!("Node updated: {}", values[0]["id"]);
            }
        })
        .connect()
        .expect("Failed to connect");

    std::thread::sleep(Duration::from_secs(30));
}
```

### Project — Join room and reorder environments

```rust
use rust_socketio::{ClientBuilder, Payload, RawClient};
use serde_json::json;
use std::time::Duration;

fn main() {
    let api_url = "http://localhost:8080";
    let session_cookie = "better-auth.session_token=<your-session-token>";

    let project = ClientBuilder::new(api_url)
        .namespace("/project")
        .opening_header("Cookie", session_cookie)
        .transport_type(rust_socketio::TransportType::Websocket)
        .on("connect", |_payload, client, _| {
            println!("Connected to /project");
            client
                .emit("join_project", json!("project-456"))
                .expect("Failed to join project");
        })
        .on("joined_project", |payload, client, _| {
            if let Payload::Text(values) = payload {
                println!("Joined room: {}", values[0]);
            }

            // Reorder environments
            client
                .emit(
                    "update_environment_positions",
                    json!({
                        "projectId": "project-456",
                        "updates": [
                            {"id": "env-1", "position": 0},
                            {"id": "env-2", "position": 1},
                            {"id": "env-3", "position": 2}
                        ]
                    }),
                )
                .expect("Failed to reorder");
        })
        .on("environments_reordered", |payload, _client, _| {
            if let Payload::Text(values) = payload {
                println!("Environments reordered: {}", values[0]);
            }
        })
        .connect()
        .expect("Failed to connect to /project");

    std::thread::sleep(Duration::from_secs(60));
}
```

### Speech — Real-time audio transcription

```rust
use rust_socketio::{ClientBuilder, Payload, RawClient};
use serde_json::json;
use std::fs;
use std::time::Duration;

fn main() {
    let api_url = "http://localhost:8080";
    let session_cookie = "better-auth.session_token=<your-session-token>";

    let speech = ClientBuilder::new(api_url)
        .namespace("/speech")
        .opening_header("Cookie", session_cookie)
        .transport_type(rust_socketio::TransportType::Websocket)
        .on("connect", |_payload, client, _| {
            println!("Connected to /speech — starting recording...");
            client
                .emit(
                    "start_recording",
                    json!({
                        "sampleRateHertz": 48000,
                        "languageCode": "es-ES"
                    }),
                )
                .expect("Failed to start recording");
        })
        .on("speech_result", |payload, _client, _| {
            if let Payload::Text(values) = payload {
                let data = &values[0];
                let is_final = data["isFinal"].as_bool().unwrap_or(false);
                let transcript = data["transcript"].as_str().unwrap_or("");
                let prefix = if is_final { "[FINAL]" } else { "[INTERIM]" };
                println!("{prefix} {transcript}");
            }
        })
        .on("speech_error", |payload, _client, _| {
            if let Payload::Text(values) = payload {
                eprintln!("Speech error: {}", values[0]["message"]);
            }
        })
        .connect()
        .expect("Failed to connect to /speech");

    // Send audio file in chunks (base64-encoded)
    if let Ok(audio_data) = fs::read("audio.webm") {
        use base64::Engine as _;
        for chunk in audio_data.chunks(4096) {
            let encoded = base64::engine::general_purpose::STANDARD.encode(chunk);
            speech
                .emit("audio_data", json!(encoded))
                .expect("Failed to send audio chunk");
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    speech
        .emit("stop_recording", json!({}))
        .expect("Failed to stop recording");

    std::thread::sleep(Duration::from_secs(5));
}
```

> **Note (Rust speech):** Add `base64 = "0.22"` to `Cargo.toml` for the base64 encoding in the speech example.

---

## Summary

### REST API Endpoints

| Module          | Base Route      | Endpoints | Auth     |
| --------------- | --------------- | --------- | -------- |
| Auth            | `api/auth`      | 8         | Mixed    |
| User            | `user`          | 4         | Required |
| Workspace       | `workspace`     | 8         | Required |
| Project         | `projects`      | 6         | Required |
| Folder          | `folders`       | 5         | Required |
| Environment     | `environments`  | 5         | Required |
| Nodes           | `agents/nodes`  | 5         | None     |
| Edges           | `agents/edges`  | 5         | None     |
| Groups          | `agents/groups` | 13        | Required |
| Chat / Sessions | `chat`          | 14        | Required |
| API Keys        | `api-keys`      | 6         | Required |
| RAG             | `rag`           | 8         | None     |
| GCS             | `gcs`           | 3         | None     |
| Invitations     | `invite`        | 16        | Mixed    |
| Notifications   | `notifications` | 7         | Required |
| Search          | `search`        | 1         | Required |
| Support         | `support`       | 2         | None     |
| Messages        | `message`       | 2         | None     |

### WebSocket Namespaces

| Namespace  | Room Pattern                  | Purpose              | Events (C->S / S->C) |
| ---------- | ----------------------------- | -------------------- | -------------------- |
| `/canvas`  | `environment:{environmentId}` | Canvas collaboration | 17 / 11              |
| `/project` | `project_{projectId}`         | Project management   | 3 / 2                |
| `/speech`  | _(no rooms)_                  | Audio transcription  | 3 / 2                |
# Leverage Rust's Type System

**Use the type system to enforce correctness at compile time. Never compromise on type safety.**

## Core Principle

In this project, we use Rust's type system as our **primary defense against bugs**. The compiler is our best tool - let it do its job.

**Type safety is non-negotiable.**

## The Four Rules

### 1. Use Newtypes for Domain Concepts

Raw primitive types lose semantic meaning. Wrap them in newtypes.

**Never do this:**
```rust
fn create_session(user_id: String, project_id: String) -> String {
    // Easy to swap user_id and project_id by accident!
    todo!()
}

// Caller can easily mix up arguments
create_session(project_id, user_id); // Compiles but WRONG
```

**Always do this:**
```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);

fn create_session(user_id: UserId, project_id: ProjectId) -> SessionId {
    todo!()
}

// Compiler catches the mistake
create_session(project_id, user_id); // ERROR: expected UserId, found ProjectId
```

### 2. Make Invalid States Unrepresentable

Use enums to model state machines. Don't use strings or booleans for states.

**Never do this:**
```rust
struct Run {
    status: String, // "pending", "running", "completed", "failed"
    started_at: Option<chrono::DateTime<Utc>>,
    completed_at: Option<chrono::DateTime<Utc>>,
    error: Option<String>,
}

// Nothing prevents: status = "running" but started_at = None
// Nothing prevents: status = "pending" but error = Some("oops")
```

**Always do this:**
```rust
enum RunStatus {
    Pending,
    Running {
        started_at: chrono::DateTime<Utc>,
    },
    Completed {
        started_at: chrono::DateTime<Utc>,
        completed_at: chrono::DateTime<Utc>,
    },
    Failed {
        started_at: chrono::DateTime<Utc>,
        failed_at: chrono::DateTime<Utc>,
        error: String,
    },
}

// Invalid states are impossible - compiler enforces this
```

### 3. Use Proper Error Types

Define domain-specific error types. Don't use strings or generic errors.

**Never do this:**
```rust
fn get_session(id: SessionId) -> Result<Session, String> {
    // String errors lose type info and are hard to match on
    Err("not found".to_string())
}

fn get_session_v2(id: SessionId) -> Result<Session, Box<dyn std::error::Error>> {
    // Box<dyn Error> makes it impossible to handle specific errors
    todo!()
}
```

**Always do this:**
```rust
// In library crates: use thiserror
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("session not found: {0}")]
    SessionNotFound(SessionId),
    #[error("duplicate session name: {0}")]
    DuplicateName(String),
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
}

fn get_session(id: SessionId) -> Result<Session, DbError> {
    // Callers can match on specific error variants
    todo!()
}

// In application code (binaries): use anyhow
async fn handle_request(id: SessionId) -> anyhow::Result<Session> {
    let session = get_session(id)
        .context("failed to query session")?;
    Ok(session)
}
```

### 4. Define Structs for All Data Structures

Every data structure should have a defined type. No anonymous tuples or untyped collections.

**Never do this:**
```rust
// Returning untyped tuples
fn get_stats() -> (u64, u64, f64) {
    (total, active, ratio) // What do these numbers mean?
}

// Using HashMap for structured data
fn get_config() -> HashMap<String, String> {
    // No compile-time guarantee about which keys exist
    todo!()
}
```

**Always do this:**
```rust
struct SessionStats {
    total: u64,
    active: u64,
    utilization_ratio: f64,
}

fn get_stats() -> SessionStats {
    SessionStats { total, active, utilization_ratio: ratio }
}

struct AppConfig {
    api_url: String,
    database_url: String,
    timeout_seconds: u64,
}

fn get_config() -> AppConfig {
    // All fields guaranteed at compile time
    todo!()
}
```

## Common Scenarios

### API Request/Response Types

**Bad:**
```rust
async fn create_session(
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let name = body["name"].as_str().unwrap(); // Panics if missing!
    todo!()
}
```

**Good:**
```rust
#[derive(Debug, Deserialize)]
struct CreateSessionRequest {
    name: String,
    project_id: ProjectId,
}

#[derive(Debug, Serialize)]
struct CreateSessionResponse {
    id: SessionId,
    name: String,
    status: String,
}

async fn create_session(
    Json(body): Json<CreateSessionRequest>,
) -> Result<Json<CreateSessionResponse>, AppError> {
    // Axum validates and deserializes automatically
    todo!()
}
```

### Database Models

**Bad:**
```rust
// Using raw SQL results without typed models
let row = sqlx::query("SELECT * FROM sessions WHERE id = $1")
    .bind(&id)
    .fetch_one(&pool)
    .await?;
let name: String = row.get("name"); // Runtime error if column doesn't exist
```

**Good:**
```rust
#[derive(Debug, sqlx::FromRow)]
struct SessionRow {
    id: Uuid,
    name: String,
    status: String,
    created_at: chrono::DateTime<Utc>,
}

let session = sqlx::query_as::<_, SessionRow>(
    "SELECT id, name, status, created_at FROM sessions WHERE id = $1"
)
    .bind(&id.0)
    .fetch_optional(&pool)
    .await?;
```

### Option vs Sentinel Values

**Bad:**
```rust
fn find_session(id: SessionId) -> Session {
    // Returns "empty" session if not found - masks the problem
    match db_lookup(id) {
        Some(s) => s,
        None => Session::default(), // Silent failure
    }
}
```

**Good:**
```rust
fn find_session(id: SessionId) -> Option<Session> {
    db_lookup(id) // Caller must handle None explicitly
}

// Or with error context
fn get_session(id: SessionId) -> Result<Session, DbError> {
    db_lookup(id).ok_or(DbError::SessionNotFound(id))
}
```

### Conversion Traits

**Bad:**
```rust
// Manual string conversion everywhere
let id_str = session.id.to_string();
let session_id = SessionId(id_str.parse().unwrap());
```

**Good:**
```rust
impl From<Uuid> for SessionId {
    fn from(id: Uuid) -> Self {
        Self(id)
    }
}

impl AsRef<Uuid> for SessionId {
    fn as_ref(&self) -> &Uuid {
        &self.0
    }
}

// Clean conversions
let session_id: SessionId = uuid.into();
```

## Advanced Type Patterns

### Typestate Pattern

Use the typestate pattern for multi-step operations:

```rust
struct Unvalidated;
struct Validated;

struct Request<State = Unvalidated> {
    data: RequestData,
    _state: std::marker::PhantomData<State>,
}

impl Request<Unvalidated> {
    fn validate(self) -> Result<Request<Validated>, ValidationError> {
        // validation logic...
        Ok(Request {
            data: self.data,
            _state: PhantomData,
        })
    }
}

impl Request<Validated> {
    fn execute(self) -> Result<Response, ExecutionError> {
        // Only validated requests can be executed
        todo!()
    }
}
```

### Builder Pattern

Use builders for complex construction:

```rust
struct SessionBuilder {
    name: String,
    project_id: ProjectId,
    timeout: Option<Duration>,
}

impl SessionBuilder {
    fn new(name: String, project_id: ProjectId) -> Self {
        Self { name, project_id, timeout: None }
    }

    fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    fn build(self) -> Session {
        Session {
            id: SessionId(Uuid::new_v4()),
            name: self.name,
            project_id: self.project_id,
            timeout: self.timeout.unwrap_or(Duration::from_secs(300)),
        }
    }
}
```

## Configuration

Ensure strict settings in `Cargo.toml`:

```toml
[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.lints.clippy]
all = "warn"
pedantic = "warn"
```

## Benefits of Strict Type Safety

### Catch Errors at Compile Time
- Wrong argument order caught immediately
- Missing fields caught at compile time
- Impossible state transitions prevented

### Self-Documenting Code
- Types serve as documentation
- Clear function contracts
- Easier to understand code

### Safer Refactoring
- Compiler catches breaking changes
- Confidence when modifying code
- Find all usages automatically

## Remember

**Types are not overhead - they're safety guarantees.**

- Newtypes for all domain concepts
- Enums for state machines
- `thiserror` for library errors, `anyhow` for binaries
- Structs for all data structures

**If the compiler complains, fix the code - the compiler is right.**

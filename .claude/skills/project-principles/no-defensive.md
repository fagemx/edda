# Avoid Defensive Programming

**Use `Result`/`Option` and `?` propagation. Don't panic or swallow errors.**

## What is Defensive Programming?

Defensive programming is the practice of adding excessive error handling, validation, and safety checks to "defend" against errors - even when those errors should naturally propagate or can't be meaningfully handled.

**In this project, we AVOID defensive programming.**

## Core Philosophy

**Only handle errors when you can meaningfully recover.**

Rust's type system and ownership model already prevent most bugs at compile time. Don't add unnecessary `.unwrap()` checks, redundant `match` arms, or swallow errors with empty `if let` blocks - this adds noise without adding value.

## The Three Rules

### 1. Only Handle Errors When You Can Meaningfully Recover

**Bad - Defensive programming:**
```rust
pub async fn create_session(
    pool: &PgPool,
    params: CreateSessionParams,
) -> Result<Session, AppError> {
    let result = match sqlx::query_as::<_, Session>(
        "INSERT INTO sessions (name, project_id) VALUES ($1, $2) RETURNING *"
    )
    .bind(&params.name)
    .bind(&params.project_id.0)
    .fetch_one(pool)
    .await
    {
        Ok(session) => session,
        Err(e) => {
            tracing::error!("Failed to create session: {}", e);
            return Err(AppError::Internal("Failed to create session".into()));
        }
    };
    Ok(result)
}
```

**Problems:**
- Catches all errors indiscriminately
- Loses error details by returning generic message
- `tracing::error!` before re-throwing adds noise
- Could be a single line with `?`

**Good - Let errors propagate:**
```rust
pub async fn create_session(
    pool: &PgPool,
    params: CreateSessionParams,
) -> Result<Session, sqlx::Error> {
    sqlx::query_as::<_, Session>(
        "INSERT INTO sessions (name, project_id) VALUES ($1, $2) RETURNING *"
    )
    .bind(&params.name)
    .bind(&params.project_id.0)
    .fetch_one(pool)
    .await
}
```

**Why good:**
- Error propagates with full details
- Caller decides how to handle (Axum converts to HTTP response)
- Simpler and more maintainable

### 2. Let Errors Bubble Up with `?`

**Bad - Catching too early:**
```rust
async fn get_user_data(pool: &PgPool, user_id: &UserId) -> Result<User, DbError> {
    let user = match sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
        .bind(&user_id.0)
        .fetch_optional(pool)
        .await
    {
        Ok(Some(user)) => user,
        Ok(None) => {
            tracing::warn!("User not found: {}", user_id.0);
            return Err(DbError::NotFound);
        }
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return Err(DbError::Sqlx(e));
        }
    };
    Ok(user)
}
```

**Problems:**
- Verbose match that just re-maps errors
- Logging at wrong level (should be at handler level)
- Better to let error propagate to where it can be handled

**Good - Let it propagate:**
```rust
async fn get_user_data(pool: &PgPool, user_id: &UserId) -> Result<User, DbError> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
        .bind(&user_id.0)
        .fetch_optional(pool)
        .await?
        .ok_or(DbError::NotFound)
}
```

**Why good:**
- `?` propagates sqlx::Error automatically (via `From` impl)
- `.ok_or()` converts None to the appropriate domain error
- Caller can decide how to handle

### 3. Trust the Type System

**Bad - Over-defensive validation:**
```rust
fn calculate_total(items: &[CartItem]) -> Result<f64, String> {
    if items.is_empty() {
        return Err("Items is empty".to_string());
    }

    let mut total = 0.0;
    for item in items {
        if item.price < 0.0 {
            return Err("Price cannot be negative".to_string());
        }
        if item.quantity == 0 {
            return Err("Quantity cannot be zero".to_string());
        }
        total += item.price * item.quantity as f64;
    }
    Ok(total)
}
```

**Problems:**
- Overly defensive validation
- The type system should ensure valid data at construction time
- Returns Result for logic that can't fail with valid types

**Good - Trust types:**
```rust
fn calculate_total(items: &[CartItem]) -> f64 {
    items.iter().map(|item| item.price * item.quantity as f64).sum()
}
```

**Why good:**
- CartItem type ensures valid price/quantity at construction
- Empty slice produces 0.0, which is correct
- If types are wrong, fix the types, not the consumer

## When to Use Explicit Error Handling

### When You Have Specific Error Recovery Logic

```rust
async fn fetch_session(client: &reqwest::Client, id: &SessionId) -> Result<Session, Error> {
    match client.get(&format!("/sessions/{}", id.0)).send().await {
        Ok(resp) if resp.status().is_success() => {
            Ok(resp.json::<Session>().await?)
        }
        Ok(resp) if resp.status() == StatusCode::NOT_FOUND => {
            // Meaningful recovery: return cached data
            get_cached_session(id).await
        }
        Ok(resp) => Err(Error::Api(resp.status())),
        Err(e) => Err(Error::Network(e)),
    }
}
```

**Why good:** Specific fallback strategy for 404s. Not just logging and re-throwing.

### When You Need to Transform the Error

```rust
async fn get_session(pool: &PgPool, id: SessionId) -> Result<Session, AppError> {
    db::find_session(pool, &id)
        .await
        .map_err(AppError::Database)?
        .ok_or(AppError::NotFound(format!("session {}", id.0)))
}
```

**Why good:** Wraps internal DbError into AppError for the API layer.

### When You Need Cleanup Logic

```rust
async fn process_upload(path: &Path) -> Result<(), Error> {
    let temp_dir = tempfile::tempdir()?;
    let result = extract_and_process(&temp_dir, path).await;
    // temp_dir is automatically cleaned up when dropped (RAII)
    result
}
```

**Why good:** Rust's RAII handles cleanup. No need for explicit try/finally.

## Project-Specific Guidelines

### Database Operations

**Bad:**
```rust
async fn create_user(pool: &PgPool, data: NewUser) -> Result<User, AppError> {
    match sqlx::query_as::<_, User>("INSERT INTO users ...")
        .bind(&data.name)
        .fetch_one(pool)
        .await
    {
        Ok(user) => Ok(user),
        Err(e) => {
            tracing::error!("Failed to create user: {}", e);
            Err(AppError::Internal("User creation failed".into()))
        }
    }
}
```

**Good:**
```rust
async fn create_user(pool: &PgPool, data: NewUser) -> Result<User, sqlx::Error> {
    sqlx::query_as::<_, User>("INSERT INTO users ...")
        .bind(&data.name)
        .fetch_one(pool)
        .await
}
```

**Why:** Database errors should fail fast. Let the error propagate with full details.

### Axum Route Handlers

**Bad:**
```rust
async fn get_session(
    Path(id): Path<String>,
    State(pool): State<PgPool>,
) -> impl IntoResponse {
    match db::find_session(&pool, &SessionId(id.parse().unwrap_or_default())).await {
        Ok(Some(session)) => (StatusCode::OK, Json(session)).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "Not found").into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Error").into_response(),
    }
}
```

**Good:**
```rust
async fn get_session(
    Path(id): Path<Uuid>,
    State(pool): State<PgPool>,
) -> Result<Json<Session>, AppError> {
    let session = db::find_session(&pool, &SessionId(id))
        .await?
        .ok_or(AppError::NotFound("session"))?;
    Ok(Json(session))
}
```

**Why:** Axum's `IntoResponse` for `Result<T, AppError>` handles status codes properly.

### External HTTP Calls

**Bad:**
```rust
async fn call_api(client: &reqwest::Client) -> Result<Data, Error> {
    let resp = match client.get("https://api.example.com/data").send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("API call failed: {}", e);
            return Err(Error::Network(e));
        }
    };
    let data = match resp.json::<Data>().await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("Failed to parse response: {}", e);
            return Err(Error::Parse(e));
        }
    };
    Ok(data)
}
```

**Good:**
```rust
async fn call_api(client: &reqwest::Client) -> Result<Data, reqwest::Error> {
    client
        .get("https://api.example.com/data")
        .send()
        .await?
        .error_for_status()?
        .json::<Data>()
        .await
}
```

**Why:** Chain `?` for clean error propagation. Caller handles the error.

## Mental Framework

Before adding explicit error handling, ask:

### 1. Can I Meaningfully Handle This Error?

- "Log and re-throw" -> Not meaningful, use `?`
- "Return default value" -> Masks problem, use `?`
- "Show user feedback" -> Meaningful (in handler layer)
- "Use fallback data" -> Meaningful

### 2. Where Should This Error Be Handled?

- In utility function -> Too early, use `?`
- In data layer -> Too early, use `?`
- In Axum handler -> Right place
- In CLI main -> Right place

### 3. Does the Type System Already Prevent This?

- Checking if `Vec` is null -> Impossible in Rust
- Checking if `String` is valid UTF-8 -> Already guaranteed
- Validating external input -> Necessary (use Deserialize)

## Common Anti-Patterns

### Anti-Pattern 1: Log and Return Err

**Never do this:**
```rust
match do_something().await {
    Ok(v) => Ok(v),
    Err(e) => {
        tracing::error!("Failed: {}", e);
        Err(e)
    }
}
```

**Why bad:** Just use `?`. Logging should happen at the handler level.

### Anti-Pattern 2: Unwrap with Fallback

**Avoid this:**
```rust
let value = config.get("key").unwrap_or(&"default".to_string());
```

**Better:**
```rust
let value = config.get("key").unwrap_or("default");
// Or make config a proper struct with typed fields
```

### Anti-Pattern 3: Redundant Option Checks

**Avoid this:**
```rust
if let Some(session) = find_session(id).await? {
    Ok(session)
} else {
    Err(Error::NotFound)
}
```

**Better:**
```rust
find_session(id).await?.ok_or(Error::NotFound)
```

## Benefits of Avoiding Defensive Programming

### Cleaner Code
- Less boilerplate
- Easier to read
- Clearer logic flow

### Better Debugging
- Errors surface with full context
- No masked failures
- Easier to identify root cause

### Idiomatic Rust
- `?` operator is Rust's way of handling errors
- Follows community conventions
- Makes code reviewable

## Remember

**Trust the type system. Trust `?` propagation. Let errors surface naturally.**

Only add explicit error handling when you have a specific, meaningful way to handle the error.

**"The best error handling is `?`."**

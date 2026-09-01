# YAGNI (You Aren't Gonna Need It)

**This is a CORE PRINCIPLE for this project.** We follow the YAGNI principle strictly to keep the codebase simple and maintainable.

## What is YAGNI?

YAGNI stands for "You Aren't Gonna Need It" - a principle that states you should not add functionality until it is actually needed, not just when you foresee that you might need it.

## Core Philosophy

**Start with the simplest solution that works, then evolve as actual needs arise.**

The enemy of good code is not bad code - it's unnecessary code. Every line of code you write:
- Must be tested
- Must be maintained
- Increases complexity
- Can introduce bugs

Therefore, only write code that solves **current, real problems**.

## The Four Rules

### 1. Don't Add Functionality Until It's Actually Needed

**Bad:**
```rust
// Adding configuration options "just in case"
struct SessionServiceConfig {
    timeout: Duration,
    retries: u32,
    cache_ttl: Duration,
    enable_metrics: bool,
    fallback_strategy: FallbackStrategy,
    max_concurrent: usize,
}

impl SessionService {
    fn new(config: SessionServiceConfig) -> Self {
        // We only use timeout right now...
        todo!()
    }
}
```

**Good:**
```rust
// Only add what you need NOW
impl SessionService {
    fn new(pool: PgPool, timeout: Duration) -> Self {
        Self { pool, timeout }
    }
}

// Add more config options later when they're actually needed
```

### 2. Start with the Simplest Solution That Works

**Bad:**
```rust
// Over-engineered abstraction for simple use case
trait DataStore<T>: Send + Sync {
    async fn get(&self, id: &str) -> Result<T>;
    async fn set(&self, id: &str, value: T) -> Result<()>;
    async fn delete(&self, id: &str) -> Result<()>;
    async fn bulk_get(&self, ids: &[&str]) -> Result<Vec<T>>;
    async fn query(&self, predicate: Box<dyn Fn(&T) -> bool>) -> Result<Vec<T>>;
}

struct InMemoryStore<T> { /* ... */ }
struct RedisStore<T> { /* ... */ }  // We don't even use Redis yet

// Only using it to store one config value
```

**Good:**
```rust
// Simple solution for current need
let current_config: Arc<RwLock<Config>> = Arc::new(RwLock::new(config));

// Add abstraction later if you need multiple storage backends
```

### 3. Avoid Premature Abstractions

**Bad:**
```rust
// Creating trait hierarchy for 2 similar functions
trait Validator {
    fn validate(&self, value: &str) -> bool;
}

struct EmailValidator;
impl Validator for EmailValidator {
    fn validate(&self, email: &str) -> bool {
        email.contains('@')
    }
}

struct PhoneValidator;
impl Validator for PhoneValidator {
    fn validate(&self, phone: &str) -> bool {
        phone.len() == 10 && phone.chars().all(|c| c.is_ascii_digit())
    }
}

struct ValidatorFactory;
impl ValidatorFactory {
    fn create(kind: &str) -> Box<dyn Validator> {
        // Factory logic...
        todo!()
    }
}
```

**Good:**
```rust
// Simple functions - add abstraction only if you need it
fn is_valid_email(email: &str) -> bool {
    email.contains('@')
}

fn is_valid_phone(phone: &str) -> bool {
    phone.len() == 10 && phone.chars().all(|c| c.is_ascii_digit())
}
```

### 4. Delete Unused Code Aggressively

**Bad:**
```rust
// Keeping "just in case" code
fn calculate_discount(price: f64, _code: Option<&str>) -> f64 {
    // This feature was removed but we kept the code
    // if let Some("SPECIAL") = code {
    //     return price * 0.5;
    // }
    price
}

// Unused utility function "might be useful someday"
fn deep_clone<T: Serialize + DeserializeOwned>(obj: &T) -> T {
    serde_json::from_str(&serde_json::to_string(obj).unwrap()).unwrap()
}
```

**Good:**
```rust
// Delete unused code - git history preserves it if you need it
fn calculate_discount(price: f64) -> f64 {
    price
}

// deep_clone function deleted - add it back if/when needed
```

## Project-Specific Examples

### Test Helpers

**Bad:**
```rust
// test_utils.rs with many unused helpers
pub fn create_mock_session() -> Session { todo!() }
pub fn create_mock_run() -> Run { todo!() }
pub fn create_mock_project() -> Project { todo!() }
pub fn setup_test_db() -> PgPool { todo!() }
pub fn mock_api_call() -> Response { todo!() }

// Only create_mock_session is actually used in tests
```

**Good:**
```rust
// test_utils.rs with only actively used helpers
pub fn create_mock_session() -> Session { todo!() }

// Add other helpers only when tests actually need them
```

### Configuration

**Bad:**
```rust
// Extensive configuration struct
struct Config {
    api_timeout: Duration,
    api_retries: u32,
    api_base_url: String,
    rate_limit_per_minute: u32,
    enable_compression: bool,
    enable_caching: bool,
    cache_strategy: CacheStrategy,
    feature_dark_mode: bool,
    feature_beta: bool,
    feature_analytics: bool,
    // 20 more feature flags we don't use...
}

// We only use api_timeout and api_base_url
```

**Good:**
```rust
// Starting minimal
struct Config {
    api_timeout: Duration,
    api_base_url: String,
}

// Grow configuration as features are added
```

### Trait Definitions

**Bad:**
```rust
// Over-specified trait with methods we don't need yet
#[async_trait]
pub trait SandboxProvider: Send + Sync {
    async fn create(&self, params: CreateParams) -> Result<Sandbox>;
    async fn destroy(&self, id: &SandboxId) -> Result<()>;
    async fn list(&self) -> Result<Vec<Sandbox>>;
    async fn resize(&self, id: &SandboxId, size: Size) -> Result<()>;
    async fn snapshot(&self, id: &SandboxId) -> Result<Snapshot>;
    async fn restore(&self, snapshot: &Snapshot) -> Result<Sandbox>;
    async fn metrics(&self, id: &SandboxId) -> Result<Metrics>;
}

// We only use create and destroy right now
```

**Good:**
```rust
// Only define what we actually use
#[async_trait]
pub trait SandboxProvider: Send + Sync {
    async fn create(&self, params: CreateParams) -> Result<Sandbox>;
    async fn destroy(&self, id: &SandboxId) -> Result<()>;
}

// Add list, resize, etc. when they're actually needed
```

### "Just in Case" Parameters

**Bad:**
```rust
// Adding optional parameters we don't use yet
async fn fetch_sessions(
    pool: &PgPool,
    page: Option<u32>,
    limit: Option<u32>,
    sort_by: Option<String>,
    sort_order: Option<SortOrder>,
    filters: Option<HashMap<String, String>>,
    include_deleted: Option<bool>,
) -> Result<Vec<Session>> {
    // Currently only using basic fetch with no params
    sqlx::query_as("SELECT * FROM sessions")
        .fetch_all(pool)
        .await
}
```

**Good:**
```rust
// Start simple
async fn fetch_sessions(pool: &PgPool) -> Result<Vec<Session>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM sessions")
        .fetch_all(pool)
        .await
}

// Add pagination when it's actually needed
```

## When to Add Complexity

Only add complexity when:

### The Need is Current and Real
```rust
// Bad: Adding caching "just in case performance becomes an issue"
// Good: Adding caching because load testing showed it's needed
```

### You Have 3+ Use Cases
```rust
// Bad: Abstracting after first use
// Good: Abstracting after third similar usage
```

### Complexity is Less Than Duplication
```rust
// Bad: Creating complex trait to avoid 3 lines of duplication
// Good: Creating trait when duplication causes real maintenance burden
```

## Mental Framework

Before adding any code, ask:

1. **Do we need this RIGHT NOW?**
   - Not "might we need it later"
   - Not "it would be nice to have"
   - RIGHT NOW

2. **What is the simplest solution?**
   - Not "what's the most elegant"
   - Not "what's the most flexible"
   - The SIMPLEST

3. **Can we delete something instead?**
   - Maybe this feature isn't needed at all
   - Maybe existing code can be simplified

## The Rule of Three

A good heuristic:

- **First time:** Write code inline
- **Second time:** Copy and paste (with awareness)
- **Third time:** Abstract into reusable function/trait

Don't abstract before the third use.

## Practical Checklist

Before committing code, ask yourself:

- [ ] Is every function/parameter actually being used?
- [ ] Could this be simpler?
- [ ] Am I building for current needs or imagined future needs?
- [ ] Can I delete any code?
- [ ] Would this code still be needed if requirements change?

If you're building for imagined future needs, STOP and simplify.

## Remember

**The best code is no code at all.**

Every line of code is a liability. Write the minimum necessary to solve the current problem well.

**"Premature optimization is the root of all evil" - Donald Knuth**

This applies to features too:
**"Premature abstraction is the root of all evil."**

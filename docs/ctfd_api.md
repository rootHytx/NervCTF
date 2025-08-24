# `ctfd_api` Module Documentation

The `ctfd_api` module is the abstraction layer for all interactions with the CTFd REST API. It provides a type-safe, ergonomic, and extensible interface for all CTFd resources, including challenges, flags, files, hints, and tags. This documentation covers the architecture, design decisions, and detailed explanations of each component and function, with a focus on aiding further development and debugging.

---

## Module Structure

- **client.rs**: The main API client, handling authentication, request construction, and response parsing.
- **endpoints/**: Submodules for each CTFd resource (challenges, flags, files, hints, tags), implemented as extension methods on `CtfdClient`.
- **models.rs**: Data models for all CTFd entities, used for serialization/deserialization.

---

## 1. `client.rs` — The `CtfdClient`

### Purpose

`CtfdClient` is the main struct for interacting with the CTFd API. It manages authentication, HTTP client configuration, and provides generic request/response methods.

### Key Fields

- `client: reqwest::Client`: Asynchronous HTTP client for most API operations.
- `blocking_client: reqwest::blocking::Client`: Used for multipart file uploads (which are not yet fully async in `reqwest`).
- `base_url: String`: The base URL of the CTFd instance.
- `api_key: String`: The API key for authentication.

### Construction

```rust
pub fn new(base_url: &str, api_key: &str) -> Result<Self>
```
- Sets up default headers (Authorization, Content-Type).
- Configures timeouts for both async and blocking clients.
- Trims trailing slashes from the base URL for consistency.

**Debugging tip:**
If authentication fails, check the API key format and ensure the `Authorization` header is correctly set.

### Core Methods

#### `request`

```rust
pub async fn request<T: Serialize + ?Sized>(
    &self,
    method: Method,
    endpoint: &str,
    body: Option<&T>,
) -> Result<Response>
```
- Constructs the full URL.
- Attaches JSON body if provided.
- Sends the request and checks for HTTP errors.
- Returns the raw response for further parsing.

#### `parse_response`

```rust
pub async fn parse_response<T: DeserializeOwned>(response: Response) -> Result<T>
```
- Attempts to parse the response as JSON.
- Handles both `{ "data": ... }` and direct object responses.
- Returns a deserialized Rust struct or an error with context.

#### `execute`

```rust
pub async fn execute<T: DeserializeOwned, B: Serialize + ?Sized>(
    &self,
    method: Method,
    endpoint: &str,
    body: Option<&B>,
) -> Result<Option<T>>
```
- Combines `request` and `parse_response`.
- For GET requests, returns the parsed object; for others, returns `None`.

#### `post_file`

```rust
pub async fn post_file<T: DeserializeOwned>(
    &self,
    endpoint: &str,
    form: Option<multipart::Form>,
) -> Result<Option<T>>
```
- Used for multipart file uploads (e.g., challenge files).
- Uses the blocking client due to async limitations.
- Returns `None` (can be extended to parse file upload responses).

#### `execute_with_params`

```rust
pub async fn execute_with_params<T: DeserializeOwned, B: Serialize + ?Sized, P: Serialize>(
    &self,
    method: Method,
    endpoint: &str,
    body: Option<&B>,
    params: &P,
) -> Result<T>
```
- Allows sending requests with query parameters.
- Useful for endpoints that support filtering or pagination.

#### `request_without_body`

```rust
pub async fn request_without_body<T: Serialize + ?Sized>(
    &self,
    method: Method,
    endpoint: &str,
    body: Option<&T>,
) -> Result<()>
```
- Used for DELETE and similar requests where no response body is expected.

---

## 2. `endpoints/` — Resource-Specific API Methods

Each file in `endpoints/` implements extension methods on `CtfdClient` for a specific CTFd resource.

### `challenges.rs`

- `get_challenges`: Fetch all challenges.
- `get_challenge`: Fetch a challenge by ID.
- `get_challenge_id`: Fetch a challenge's ID by name.
- `create_challenge`: Create a new challenge.
- `update_challenge`: Update an existing challenge.
- `delete_challenge`: Delete a challenge.
- `get_challenge_files_endpoint`: Get files for a challenge.
- `get_challenge_flags_endpoint`: Get flags for a challenge.
- `get_challenge_tags_endpoint`: Get tags for a challenge.
- `get_challenge_hints_endpoint`: Get hints for a challenge.

**Design note:**
All methods use the generic `execute` or `request_without_body` methods from `CtfdClient`, ensuring consistent error handling and response parsing.

### `flags.rs`, `files.rs`, `hints.rs`, `tags.rs`

Each of these modules provides:
- `get_*`: Fetch all entities of that type.
- `get_*`: Fetch a specific entity by ID.
- `create_*`: Create a new entity.
- `delete_*`: Delete an entity by ID.

**Debugging tip:**
If you encounter a 404 or 400 error, check that the ID exists and is correct, and that the API endpoint matches the CTFd version you are using.

---

## 3. `models.rs` — Data Models

Defines all Rust structs and enums for CTFd entities, including:
- `Challenge`, `FlagContent`, `Tag`, `Hint`, `File`, etc.
- Enum variants for challenge/flag types, state, etc.
- Dependency management structs (`RequirementsQueue`, `ChallengeWaiting`).

**Serialization/Deserialization:**
- Uses `serde` with custom attributes for field renaming and untagged enums.
- Supports both simple and advanced formats for flags and tags.

**Tricky aspects:**
- Untagged enums allow both string and object representations, which can be error-prone if the input data is inconsistent.
- Default values (e.g., for `version`) ensure backward compatibility.

---

## 4. Error Handling and Debugging

- All API errors include the HTTP method, endpoint, and error text.
- Deserialization errors are surfaced with the full error message and the offending data.
- For multipart uploads, ensure the file exists and is accessible; otherwise, the blocking client will return an error.

**Best Practices:**
- Always check for `Option` values when working with IDs returned from the API.
- Use the provided error context to quickly locate issues in API interactions.
- When extending the API, follow the established patterns for request construction and response parsing.

---

## 5. Extending the API

- To add a new endpoint, create a new method in the appropriate `endpoints/*.rs` file.
- Use the generic `execute` or `request_without_body` methods for consistent behavior.
- Update `models.rs` with any new data structures required for the new endpoint.

---

## 6. Example Usage

```rust
let client = CtfdClient::new("https://ctfd.example.com", "API_KEY")?;
let challenges = client.get_challenges().await?;
for challenge in challenges.unwrap_or_default() {
    println!("Challenge: {} ({})", challenge.name, challenge.category);
}
```

---

## 7. Common Pitfalls

- **Schema mismatches:** Ensure your models match the CTFd API version you are targeting.
- **Multipart uploads:** Use absolute or correct relative paths for files; errors here are often due to missing files.
- **Async/blocking interop:** File uploads are blocking; do not call them from within a non-blocking context unless necessary.

---

## 8. Summary Table

| File/Module         | Responsibility                        | Key Functions/Structs                |
|---------------------|---------------------------------------|--------------------------------------|
| client.rs           | API client, request/response handling | CtfdClient, request, execute, etc.   |
| endpoints/challenges| Challenge API methods                 | get_challenges, create_challenge, ...|
| endpoints/flags     | Flag API methods                      | get_flags, create_flag, ...          |
| endpoints/files     | File API methods                      | get_files, create_file, ...          |
| endpoints/hints     | Hint API methods                      | get_hints, create_hint, ...          |
| endpoints/tags      | Tag API methods                       | get_tags, create_tag, ...            |
| models.rs           | Data models for all entities          | Challenge, FlagContent, Tag, ...     |

---

## 9. Further Reading

- [CTFd API Documentation](https://ctfd.io/api/v1)
- [Reqwest Documentation](https://docs.rs/reqwest/)
- [Serde Documentation](https://serde.rs/)

---

If you need more detailed documentation for a specific endpoint or function, or want code-level comments, please specify!

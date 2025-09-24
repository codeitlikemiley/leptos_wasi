# Counter

A Leptos application running as a WASI Component

## Adding Server Functions

Server functions allow you to run code on the server from client interactions. To add a new server function:

1. Define the function with `#[server]` attribute in your component file
2. Register it in `src/server.rs` with `.with_server_fn::<YourFunction>()`

Example:
```rust
#[server]
pub async fn my_server_function() -> Result<String, ServerFnError> {
    // Your server-side logic here
    Ok("Result".to_string())
}
```

## Troubleshooting

### Build Errors
- Ensure you have `wasm32-wasip2` target installed
- Check that all dependencies in Cargo.toml are correct versions

### Runtime Errors
- Verify wasmtime is installed and up to date
- Check that the `--dir=target/site::/` flag is present in serve.sh
- Ensure static files are being built to `target/site/public/`

### Server Functions Not Working
- Confirm the function is registered in `src/server.rs`
- Check browser console for any client-side errors
- Verify the server is receiving requests (check terminal output)

## License

This project is licensed under the MIT License - see the LICENSE file for details.
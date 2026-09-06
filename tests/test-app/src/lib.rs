pub mod app;
pub mod server;

#[cfg(test)]
mod tests {
    use super::app::App;

    #[test]
    fn route_table_is_valid() {
        leptos_wasi::validate_route_table(App, None, || {})
            .expect("route table should be valid");
    }
}

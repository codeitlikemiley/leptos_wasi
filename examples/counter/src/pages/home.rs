use leptos::prelude::*;

#[component]
pub fn Home() -> impl IntoView {
    let increment_star = ServerAction::<UpdateCount>::new();

    let count = Resource::new(move || increment_star.version().get(), |_| get_count());

    view! {
        <div class="min-h-screen bg-[#1a2332] flex items-center justify-center p-4">
            <div class="bg-[#263343] rounded-xl shadow-2xl p-8 md:p-12 max-w-md w-full border border-[#3a4a5c]">
                <div class="text-center space-y-8">
                    // Header
                    <div class="space-y-2">
                        <div class="flex items-center justify-center gap-3 mb-4">
                            // WasmCloud logo with modern styling
                            <div class="w-10 h-10 bg-[#00d4aa] rounded-lg flex items-center justify-center">
                                <span class="text-[#1a2332] font-bold text-xl">W</span>
                            </div>
                            <h1 class="text-3xl md:text-4xl font-medium text-white">
                                "Counter App"
                            </h1>
                        </div>
                        <p class="text-[#8b9cb8] text-sm">
                            "Powered by Leptos + WASI Component"
                        </p>
                    </div>

                    // Counter Display
                    <div class="relative">
                        <div class="bg-[#1a2332] rounded-lg p-8 border border-[#3a4a5c]">
                            <div class="text-5xl md:text-6xl font-light text-white tabular-nums">
                                <Suspense fallback=|| {
                                    view! { "..." }
                                }>{move || {
                                    count.get()
                                        .and_then(|result| result.ok())
                                        .map(|c| c.to_string())
                                        .unwrap_or("0".to_string())
                                }}</Suspense>
                            </div>
                            <div class="text-[#8b9cb8] text-sm mt-2 uppercase tracking-wider">
                                "Stars Given"
                            </div>
                        </div>

                        // Loading indicator overlay
                        <Show when=move || increment_star.pending().get()>
                            <div class="absolute inset-0 flex items-center justify-center bg-[#1a2332]/50 rounded-lg">
                                <div class="animate-spin rounded-full h-8 w-8 border-2 border-transparent border-t-[#00d4aa]"></div>
                            </div>
                        </Show>
                    </div>

                    // Button
                    <ActionForm action=increment_star>
                        <button
                            disabled=move || increment_star.pending().get()
                            class="w-full rounded-lg bg-[#00d4aa] px-6 py-3 text-[#1a2332] font-medium transition-all duration-200 hover:bg-[#00b894] active:scale-[0.98] disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:bg-[#00d4aa]"
                        >
                            {move || if increment_star.pending().get() {
                                "Updating..."
                            } else {
                                "Star me!"
                            }}
                        </button>
                    </ActionForm>

                    // Status indicators
                    <div class="flex items-center justify-center gap-2 text-xs">
                        <div class={move || {
                            if increment_star.pending().get() {
                                "w-2 h-2 rounded-full bg-[#00d4aa] animate-pulse"
                            } else {
                                "w-2 h-2 rounded-full bg-[#00d4aa]"
                            }
                        }}>
                        </div>
                        <span class="text-[#8b9cb8] uppercase tracking-wider">
                            {move || {
                                if increment_star.pending().get() {
                                    "Syncing"
                                } else {
                                    "Ready"
                                }
                            }}
                        </span>
                    </div>

                    // Footer info
                    <div class="pt-4 border-t border-[#3a4a5c]">
                        <p class="text-[#8b9cb8] text-xs">
                            "Running on Leptos WASI"
                        </p>
                    </div>
                </div>
            </div>
        </div>
    }
}

#[server]
pub async fn update_count() -> Result<(), ServerFnError> {
    use wasi::filesystem::{preopens::get_directories, types::{DescriptorFlags, OpenFlags, PathFlags}};

    println!("User requested an update to the store");
    let updated_value = get_count().await? + 1;
    let directories = get_directories();
    let (fd, _) = directories.first().expect("no directory given");

    fd
        .open_at(PathFlags::empty(), "store.txt", OpenFlags::CREATE, DescriptorFlags::WRITE)
        .map(|fd| {
            let stream = fd.write_via_stream(0).expect("failed to open a stream to write");
            stream.blocking_write_and_flush(updated_value.to_string().as_bytes()).expect("could not write to the store");
            ()
        })
        .map_err(|err| {
            ServerFnError::ServerError(err.message().to_string())
        })
}

#[server]
pub async fn get_count() -> Result<u64, ServerFnError> {
    use wasi::filesystem::{preopens::get_directories, types::{DescriptorFlags, OpenFlags, PathFlags}};

    println!("Getting the store");
    let directories = get_directories();
    let (fd, _) = directories.first().expect("no directory given");

    match fd.open_at(PathFlags::empty(), "store.txt", OpenFlags::CREATE, DescriptorFlags::READ) {
        Err(err) => {
            println!("could not open store for reading");
            println!("reason: {}", err.message());
            Ok(0)
        },
        Ok(fd) => {
            let file_size = fd.stat().expect("should be able to stat").size;
            match fd.read_via_stream(0) {
                Err(err) => {
                    println!("could not open stream to store");
                    println!("reason: {}", err.message());
                    Ok(0)
                },
                Ok(stream) => {
                    let mut store: Vec<u8> = Vec::new();
                    loop {
                        if store.len() as u64 >= file_size {
                            break;
                        }

                        match stream.blocking_read(256) {
                            Err(_) => return Ok(0),
                            Ok(data) => {
                                store.extend(data);
                            }
                        }
                    }
                    let result = String::from_utf8(store).expect("no utf-8");
                    Ok(result.parse::<u64>().unwrap_or(0))
                }
            }
        }
    }
}
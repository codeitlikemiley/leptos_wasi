import re
import os
import sys

# Paths to the 9 packages relative to leptos_src
pkg_paths = {
    "throw_error": "any_error/Cargo.toml",
    "any_spawner": "any_spawner/Cargo.toml",
    "hydration_context": "hydration_context/Cargo.toml",
    "leptos": "leptos/Cargo.toml",
    "leptos_meta": "meta/Cargo.toml",
    "leptos_router": "router/Cargo.toml",
    "leptos_macro": "leptos_macro/Cargo.toml",
    "leptos_integration_utils": "integrations/utils/Cargo.toml",
    "server_fn": "server_fn/Cargo.toml"
}

leptos_src = "/Volumes/goldcoders/leptos_src"
leptos_wasi = "/Volumes/goldcoders/leptos_wasi"

# Read version of each package from leptos_src
versions = {}
for pkg, rel_path in pkg_paths.items():
    path = os.path.join(leptos_src, rel_path)
    with open(path, "r") as f:
        content = f.read()
    # Find version = "..." in [package] section
    match = re.search(r'\[package\]\s*\n(?:.*\n)*?version\s*=\s*"([^"]+)"', content)
    if not match:
        print(f"Error: version not found in {path}")
        sys.exit(1)
    versions[pkg] = match.group(1)

# Also extract wasm-bindgen version from the workspace Cargo.toml of leptos_src
workspace_path = os.path.join(leptos_src, "Cargo.toml")
with open(workspace_path, "r") as f:
    workspace_content = f.read()
wasm_match = re.search(r'wasm-bindgen\s*=\s*\{\s*(?:default-features\s*=\s*false,\s*)?version\s*=\s*"([^"]+)"', workspace_content)
if wasm_match:
    versions["wasm-bindgen"] = wasm_match.group(1)
else:
    print("Warning: wasm-bindgen version not found in workspace Cargo.toml")

print("Versions read from leptos_src:")
for pkg, ver in versions.items():
    print(f"  {pkg}: {ver}")

# Files to update
files_to_update = [
    os.path.join(leptos_wasi, "Cargo.toml"),
    os.path.join(leptos_wasi, "tests/test-app/Cargo.toml"),
    os.path.join(leptos_wasi, "examples/counter/Cargo.toml"),
    os.path.join(leptos_wasi, "examples/spin-counter/Cargo.toml")
]

for filepath in files_to_update:
    if not os.path.exists(filepath):
        continue
    print(f"Updating {filepath}...")
    with open(filepath, "r") as f:
        content = f.read()
    
    new_content = content
    for pkg, ver in versions.items():
        if pkg == "wasm-bindgen":
            # Match wasm-bindgen = "=..." or wasm-bindgen = "..."
            wasm_ver = ver.lstrip("=")
            prefix = "=" if wasm_ver.count('.') == 2 else ""
            pattern1 = r'(wasm-bindgen\s*=\s*")=?[^"]*(")'
            new_content = re.sub(pattern1, r'\g<1>' + prefix + wasm_ver + r'\2', new_content)
            # Match wasm-bindgen = { version = "=..." or wasm-bindgen = { version = "..."
            pattern2 = r'(wasm-bindgen\s*=\s*\{\s*version\s*=\s*")=?[^"]*(")'
            new_content = re.sub(pattern2, r'\g<1>' + prefix + wasm_ver + r'\2', new_content)
        else:
            # Match e.g. leptos = { version = "..." or leptos = { version = "=..."
            pattern = r'(' + re.escape(pkg) + r'\s*=\s*\{\s*version\s*=\s*")[^"]*(")'
            new_content = re.sub(pattern, r'\g<1>=' + ver + r'\2', new_content)
    
    with open(filepath, "w") as f:
        f.write(new_content)

print("Sync completed successfully.")

import re
import sys

def main():
    if len(sys.argv) < 3:
        print("Usage: python3 stub_placeholder.py <input.wat> <output.wat>")
        sys.exit(1)
        
    wat_path = sys.argv[1]
    out_path = sys.argv[2]
    
    with open(wat_path, "r") as f:
        content = f.read()
    
    # 1. Parse all types
    # e.g., (type (;5;) (func (param i32 i32 i32) (result i32)))
    type_map = {}
    for line in content.splitlines():
        if "(type" in line:
            idx_match = re.search(r'\(type\s+\(;(\d+);\)', line)
            if idx_match:
                idx = int(idx_match.group(1))
                res_match = re.search(r'\(result\s+(\w+)\)', line)
                res_type = res_match.group(1) if res_match else None
                type_map[idx] = res_type

    # 2. Find and replace all imports from __wbindgen_placeholder__ and __wbindgen_externref_xform__
    import_pattern = re.compile(
        r'^\s*\(import\s+"__wbindgen_[\w]+"\s+"[^"]+"\s+\(func\s+(\$[\w\.]+)\s*(?:\(;\d+;\))?\s+\(type\s+(\d+)\)\)\)$',
        re.MULTILINE
    )
    
    stubs = []
    
    def replace_import(match):
        func_name = match.group(1)
        type_idx = int(match.group(2))
        
        ret_type = type_map.get(type_idx)
        
        body = ""
        if ret_type == "i32":
            body = "i32.const 0"
        elif ret_type == "i64":
            body = "i64.const 0"
        elif ret_type == "f32":
            body = "f32.const 0.0"
        elif ret_type == "f64":
            body = "f64.const 0.0"
            
        stub = f"  (func {func_name} (type {type_idx})\n    {body}\n  )"
        stubs.append(stub)
        return ""

    new_content, count = import_pattern.subn(replace_import, content)
    print(f"Removed {count} imports from __wbindgen_* modules.")
    
    # 3. Append stubs to the end of the module
    last_paren = new_content.rfind(")")
    if last_paren == -1:
        print("Error: Could not find closing parenthesis of module.")
        sys.exit(1)
        
    stubs_text = "\n\n  ;; --- STUBBED BROWSER IMPORTS ---\n" + "\n".join(stubs) + "\n"
    new_content = new_content[:last_paren] + stubs_text + new_content[last_paren:]
    
    with open(out_path, "w") as f:
        f.write(new_content)
    print(f"Wrote stubbed WAT to {out_path}.")

if __name__ == "__main__":
    main()

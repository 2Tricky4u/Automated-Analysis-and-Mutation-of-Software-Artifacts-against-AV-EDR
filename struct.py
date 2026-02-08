#!/usr/bin/env python3
import json
import sys
from typing import Optional

def type_to_str(t: dict) -> str:
    """Pretty-print a rustdoc JSON Type into a Rust-ish string."""
    if not isinstance(t, dict) or not t:
        return "<?>"

    # Most variants are one-key enums, e.g. {"primitive":"u32"} or {"resolved_path": {...}}
    if "primitive" in t:
        return t["primitive"]

    if "generic" in t:
        return t["generic"]

    if "resolved_path" in t:
        rp = t["resolved_path"]
        path = rp.get("path", "<?>")
        # Normalize leading :: noise a bit (keep it if you want fully-qualified)
        path_str = str(path)

        args = rp.get("args")
        if args:
            # args: {"angle_bracketed": {"args":[...], "constraints":[...]}}
            ab = args.get("angle_bracketed") if isinstance(args, dict) else None
            if ab and "args" in ab:
                rendered = []
                for a in ab.get("args", []):
                    # Each arg can be {"type": {...}} or others (lifetimes/consts)
                    if isinstance(a, dict) and "type" in a:
                        rendered.append(type_to_str(a["type"]))
                    elif isinstance(a, dict) and "lifetime" in a:
                        rendered.append(a["lifetime"])
                    elif isinstance(a, dict) and "const" in a:
                        rendered.append(str(a["const"]))
                    else:
                        rendered.append(str(a))
                return f"{path_str}<{', '.join(rendered)}>"
        return path_str

    if "borrowed_ref" in t:
        br = t["borrowed_ref"]
        lt = br.get("lifetime")
        mut = "mut " if br.get("is_mutable") else ""
        inner = type_to_str(br.get("type", {}))
        if lt:
            return f"&{lt} {mut}{inner}"
        return f"&{mut}{inner}"

    if "raw_pointer" in t:
        rp = t["raw_pointer"]
        mut = "mut " if rp.get("is_mutable") else "const "
        inner = type_to_str(rp.get("type", {}))
        return f"*{mut}{inner}"

    if "tuple" in t:
        elems = t["tuple"]
        if not elems:
            return "()"
        return "(" + ", ".join(type_to_str(e) for e in elems) + ")"

    if "slice" in t:
        return "[" + type_to_str(t["slice"]) + "]"

    if "array" in t:
        arr = t["array"]
        inner = type_to_str(arr.get("type", {}))
        length = arr.get("len")
        return f"[{inner}; {length}]"

    if "dyn_trait" in t:
        dt = t["dyn_trait"]
        traits = []
        for tr in dt.get("traits", []):
            # {"trait": {"path": "...", "id":..., "args":...}, "generic_params": [...]}
            tr_obj = tr.get("trait", {})
            traits.append(tr_obj.get("path", "Trait"))
        lt = dt.get("lifetime")
        if lt:
            traits.append(str(lt))
        return "dyn " + " + ".join(traits)

    if "impl_trait" in t:
        it = t["impl_trait"]
        traits = []
        for tr in it.get("traits", []):
            tr_obj = tr.get("trait", {})
            traits.append(tr_obj.get("path", "Trait"))
        return "impl " + " + ".join(traits)

    # Some less common ones exist (qualified_path, infer, etc.). Fallback:
    if len(t) == 1:
        k = next(iter(t.keys()))
        return f"<{k}>"
    return "<?>"

def should_skip_file(fn: str) -> bool:
    f = fn.replace("\\", "/").lower()

    # Skip build-script generated Rust (prost/tonic)
    if "/target-" in f and "/debug/build/" in f and "/out/" in f:
        return True
    if "/target/" in f and "/debug/build/" in f and "/out/" in f:
        return True

    # Optional: skip anything under target (more aggressive)
    # if "/target-" in f or "/target/" in f:
    #     return True

    return False

def extract_field_ids(struct_obj: dict) -> list:
    """
    rustdoc JSON: item.inner.struct.kind is enum-like:
      {"plain": {"fields": [...], ...}}
      {"tuple": [...]}   # item IDs for fields
      {"unit": {}}
    """
    kind = struct_obj.get("kind", {})
    if "plain" in kind:
        # NOTE: in your scheduler.json the shape is kind.plain.fields
        return kind["plain"].get("fields", [])
    if "tuple" in kind:
        return kind.get("tuple", [])
    return []

def module_path(paths: dict, item_id) -> Optional[str]:
    entry = paths.get(item_id) or paths.get(str(item_id))
    if not isinstance(entry, dict):
        return None
    p = entry.get("path")
    if isinstance(p, list) and p:
        return "::".join(str(x) for x in p)
    return None

def filename(item: dict) -> Optional[str]:
    span = item.get("span")
    if isinstance(span, dict):
        fn = span.get("filename")
        if fn:
            return str(fn)
    return None

def main(path: str) -> int:
    with open(path, "r", encoding="utf-8") as f:
        krate = json.load(f)

    index = krate.get("index", {})
    paths = krate.get("paths", {})

    printed_any = False

    for item in index.values():
        inner = item.get("inner", {})
        struct_obj = inner.get("struct")
        if not struct_obj:
            continue

        struct_name = item.get("name")
        if not struct_name:
            continue

        iid = item.get("id")
        mp = module_path(paths, iid) or struct_name
        fn = filename(item) or "<unknown>"

        if fn != "<unknown>" and should_skip_file(fn):
            continue

        field_ids = extract_field_ids(struct_obj)

        print(f"struct {mp}  ({fn})")
        printed_any = True

        for i, fid in enumerate(field_ids):
            # JSON keys can be strings; tolerate both
            fitem = index.get(fid) or index.get(str(fid))
            if not fitem:
                # stripped/missing field
                fname = f"_{i}"
                print(f"  {fname}: <?>")
                continue

            fname = fitem.get("name")
            if fname is None:
                # tuple struct field has no name
                fname = f"_{i}"

            fin = fitem.get("inner", {})
            sf = fin.get("struct_field")
            if not sf:
                print(f"  {fname}: <?>")
                continue

            print(f"  {fname}: {type_to_str(sf)}")

        print()

    if not printed_any:
        print("No structs found in this rustdoc JSON.")
    return 0

if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <path-to-rustdoc-json>")
        sys.exit(1)
    sys.exit(main(sys.argv[1]))

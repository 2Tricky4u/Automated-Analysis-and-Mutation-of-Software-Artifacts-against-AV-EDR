import re
from datetime import datetime
import pandas as pd
import streamlit as st
import hashlib
import colorsys

def root_of_target(t: str) -> str:
    if not isinstance(t, str) or not t.strip():
        return "unknown"
    return t.split("::", 1)[0]

def stable_pastel_rgba(key: str, alpha: float = 0.18) -> str:
    """
    Deterministic pastel RGBA from a string key.
    alpha in [0..1] controls "opacity" (lower = more subtle).
    """
    h = hashlib.sha1(key.encode("utf-8")).digest()
    hue = int.from_bytes(h[:2], "big") / 65535.0
    lightness = 0.92
    saturation = 0.45
    r, g, b = colorsys.hls_to_rgb(hue, lightness, saturation)
    return f"rgba({int(r*255)}, {int(g*255)}, {int(b*255)}, {alpha})"

def stable_pastel_hex(key: str) -> str:
    """
    Deterministic pastel color from a string key.
    Pastel-ish HLS: high lightness, moderate saturation.
    """
    h = hashlib.sha1(key.encode("utf-8")).digest()
    hue = int.from_bytes(h[:2], "big") / 65535.0  # 0..1
    lightness = 0.90
    saturation = 0.45
    r, g, b = colorsys.hls_to_rgb(hue, lightness, saturation)
    return "#{:02x}{:02x}{:02x}".format(int(r * 255), int(g * 255), int(b * 255))

def style_by_target_root(row):
    root = row.get("target_root", "unknown")
    bg = stable_pastel_rgba(str(root), alpha=0.18)  # tune alpha here
    return [f"background-color: {bg}"] * len(row)

ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")

# Parse the first two fields reliably after ANSI is removed
HEAD_RE = re.compile(
    r"^(?P<ts>\d{4}-\d{2}-\d{2}T[0-9:.]+Z)\s+"
    r"(?P<level>[A-Z]+)\s+"
    r"(?P<rest>.*)$"
)

# A "target-looking" module path (crate::module::submodule...)
# We’ll take the LAST one before a ":".
TARGET_RE = re.compile(
    r"("
    r"[A-Za-z_]\w*(?:::[A-Za-z_]\w*)+"   # crate::module::submodule
    r"|"
    r"scheduler"                         # bare root target
    r")(?=:\s)"
)

def parse_log_text(text: str) -> pd.DataFrame:
    rows = []
    for i, line in enumerate(text.splitlines(), start=1):
        raw = line.rstrip("\n")
        if not raw.strip():
            continue

        clean = ANSI_RE.sub("", raw)

        m = HEAD_RE.match(clean)
        if not m:
            rows.append({
                "line": i, "timestamp": None, "level": None,
                "target": None, "message": clean, "raw": raw,
                "parsed": False,
            })
            continue

        ts = m.group("ts")
        try:
            dt = datetime.fromisoformat(ts.replace("Z", "+00:00"))
        except Exception:
            dt = None

        rest = m.group("rest")

        # Find last module-path-like token before ": "
        targets = TARGET_RE.findall(rest)
        target = targets[-1] if targets else None

        # Message: strip everything up through "<target>: "
        msg = rest
        if target:
            # split on the last occurrence of "target: "
            marker = f"{target}: "
            idx = msg.rfind(marker)
            if idx != -1:
                msg = msg[idx + len(marker):]

        rows.append({
            "line": i,
            "timestamp": dt,
            "level": m.group("level"),
            "target": target,
            "message": msg,
            "raw": raw,      # original with ANSI (for display)
            "parsed": True,
        })

    return pd.DataFrame(rows)

st.set_page_config(page_title="Log Viewer", layout="wide")
st.title("Log Viewer (paste or upload)")

mode = st.radio("Input", ["Paste", "Upload file"], horizontal=True)

text = ""
if mode == "Paste":
    text = st.text_area(
        "Paste logs here",
        height=240,
        placeholder="Paste your log lines here..."
    )
else:
    uploaded = st.file_uploader("Upload a log file", type=None)
    if uploaded is not None:
        text = uploaded.getvalue().decode("utf-8", errors="replace")

if not text.strip():
    st.info("Provide logs (paste text or upload a file) to begin.")
    st.stop()

df = parse_log_text(text)

df["target_root"] = df["target"].apply(root_of_target)

# Sidebar filters
st.sidebar.header("Filters")

levels = sorted([lvl for lvl in df["level"].dropna().unique()],
                key=lambda x: ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"].index(x)
                if x in ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"] else 999)

selected_levels = st.sidebar.multiselect("Level", levels, default=levels)

targets = sorted([t for t in df["target"].dropna().unique()])
selected_targets = st.sidebar.multiselect("Target (module path)", targets, default=targets)

search = st.sidebar.text_input("Search (substring / regex)")
regex_mode = st.sidebar.checkbox("Regex search", value=False)
show_unparsed = st.sidebar.checkbox("Show unparsed lines", value=True)

filtered = df.copy()

if selected_levels:
    filtered = filtered[filtered["level"].isin(selected_levels) | filtered["level"].isna()]

if selected_targets:
    filtered = filtered[filtered["target"].isin(selected_targets) | filtered["target"].isna()]

if not show_unparsed:
    filtered = filtered[filtered["parsed"] == True]

if search.strip():
    if regex_mode:
        try:
            rx = re.compile(search)
            filtered = filtered[filtered["raw"].apply(lambda s: bool(rx.search(s)))]
        except re.error as e:
            st.sidebar.error(f"Invalid regex: {e}")
    else:
        filtered = filtered[filtered["raw"].str.contains(search, case=False, na=False)]

# Summary + output
c1, c2, c3 = st.columns(3)
c1.metric("Total lines", len(df))
c2.metric("Parsed lines", int(df["parsed"].sum()))
c3.metric("Shown", len(filtered))

with st.expander("Target frequency"):
    freq = df[df["target"].notna()].groupby("target").size().sort_values(ascending=False)
    st.dataframe(freq.rename("count").to_frame(), width="stretch")

st.subheader("Filtered logs")

view = filtered[["line", "timestamp", "level", "target", "message", "target_root"]].copy()

styled = (
    view.style
    .apply(style_by_target_root, axis=1)
    .hide(axis="columns", subset=["target_root"])  # keep for coloring, don't show
)

st.dataframe(styled, width="stretch", height=520)


st.subheader("Raw (filtered)")
st.text_area(
    "Raw logs",
    value="\n".join(filtered["raw"].tolist()),
    height=260,
    label_visibility="collapsed",
)

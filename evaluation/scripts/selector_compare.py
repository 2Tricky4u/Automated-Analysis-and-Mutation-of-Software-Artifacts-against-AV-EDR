#!/usr/bin/env python3
"""
Selector Comparison Figures from EvalDataset JSONs

Loads 2-4 EvalDataset JSON files (one per selector strategy) and generates
7 thesis-quality comparison figures + 1 LaTeX summary table.

Usage:
    python evaluation/scripts/selector_compare.py \
        --coverage evaluation/data/coverage_eval.json \
        --ga       evaluation/data/ga_eval.json \
        --random   evaluation/data/random_eval.json \
        --token    evaluation/data/token_eval.json \
        --outdir   evaluation/figures/compare

Optional flags:
    --window 5       Rolling mean window size (default: 5)
    --only S1,S4     Generate only specified figures

Requirements:
    pip install matplotlib numpy
    Optional: pip install seaborn (enhanced heatmap)
"""

import argparse
import json
import os
import sys
from collections import Counter

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import matplotlib.patches as mpatches
import numpy as np

# ── Thesis-quality defaults (from plots.py) ─────────────────────────────────
plt.rcParams.update({
    'font.size': 10,
    'font.family': 'serif',
    'axes.labelsize': 11,
    'axes.titlesize': 12,
    'xtick.labelsize': 9,
    'ytick.labelsize': 9,
    'legend.fontsize': 9,
    'figure.figsize': (7, 4.5),
    'figure.dpi': 300,
    'savefig.dpi': 300,
    'axes.grid': True,
    'grid.alpha': 0.3,
})

# ── Selector styling ────────────────────────────────────────────────────────
SELECTOR_STYLE = {
    'Coverage': ('#2196F3', 'o'),
    'Fuzzer':   ('#FF9800', 's'),
    'Token':    ('#4CAF50', 'D'),
    'Random':   ('#9E9E9E', '^'),
}

SELECTOR_MAP = {
    'coverage': 'Coverage',
    'ga': 'Fuzzer',
    'token': 'Token',
    'random': 'Random',
}

# Canonical order for consistent display
SELECTOR_ORDER = ['Coverage', 'Fuzzer', 'Token', 'Random']

# Behavioral token prefixes (exclude input-derived: module, mutation)
BEHAVIORAL_PREFIXES = ['api', 'api_arg', 'api_ret', 'seq2', 'image',
                       'etw', 'etw_event', 'audit', 'net', 'exit_code']

# All 10 AST mutations
ALL_AST_MUTATIONS = [
    'ast.decon_rounds', 'ast.fill_pattern', 'ast.protection_transition',
    'ast.timing_pattern', 'ast.benign_preamble', 'ast.exec_decoy',
    'ast.api_sequence_obfuscation', 'ast.benign_syscall_insert',
    'ast.const_obfuscation', 'ast.string_xor',
]


# ── Helpers ──────────────────────────────────────────────────────────────────

def load_datasets(args) -> dict:
    """Load EvalDataset JSON files from CLI args. Returns {name: parsed_json}."""
    datasets = {}
    for key, name in SELECTOR_MAP.items():
        path = getattr(args, key, None)
        if path:
            with open(path) as f:
                datasets[name] = json.load(f)
    if len(datasets) < 2:
        print("Error: at least 2 dataset files required.", file=sys.stderr)
        sys.exit(1)
    return datasets


def ordered_names(datasets: dict) -> list:
    """Return selector names in canonical order, filtered to those present."""
    return [s for s in SELECTOR_ORDER if s in datasets]


def rolling_mean(values, window):
    """Compute rolling mean. Falls back to raw values if len < window."""
    if len(values) < window:
        return list(values)
    kernel = np.ones(window) / window
    padded = np.convolve(values, kernel, mode='valid')
    # Pad start with raw values so length matches
    prefix = list(values[:window - 1])
    return prefix + list(padded)


def save_fig(fig, outdir, name):
    """Save figure as PDF + PNG, then close."""
    fig.savefig(os.path.join(outdir, f'{name}.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, f'{name}.png'), bbox_inches='tight')
    plt.close(fig)
    print(f"  Saved {name}.pdf / .png")


def get_ast_mutations(round_data):
    """Extract AST mutation names from a round's mutations list."""
    return [m for m in round_data.get('mutations', []) if m.startswith('ast.')]


def get_behavioral_tokens(token_matrix):
    """Extract behavioral tokens (exclude module/mutation prefixes)."""
    return [t for t in token_matrix.get('tokens', [])
            if t.split(':')[0] in BEHAVIORAL_PREFIXES]


# ── S1: Evasion Score Trajectory ─────────────────────────────────────────────

def plot_s1_evasion_trajectory(datasets, outdir, window=5):
    """Line plot of evasion_score over rounds with rolling mean."""
    names = ordered_names(datasets)
    fig, ax = plt.subplots(figsize=(8, 4.5))

    for name in names:
        rounds = datasets[name]['rounds']
        xs = [r['round_number'] for r in rounds]
        ys = [r.get('evasion_score') or 0 for r in rounds]

        color, marker = SELECTOR_STYLE[name]

        # Raw scores as faint scatter
        ax.scatter(xs, ys, color=color, alpha=0.15, s=12, zorder=2)

        # Rolling mean line
        rm = rolling_mean(ys, window)
        ax.plot(xs[:len(rm)], rm, color=color, marker=marker, markersize=3,
                linewidth=1.5, label=name, zorder=3)

    # Evasion threshold
    ax.axhline(y=0.6, color='#E53935', linestyle='--', linewidth=0.8,
               alpha=0.6, label='Evasion threshold (0.6)')

    ax.set_xlabel('Round Number')
    ax.set_ylabel('Evasion Score')
    ax.set_title('Evasion Score Trajectory by Selector Strategy')
    ax.set_ylim(-0.05, 1.05)
    ax.legend(loc='upper left', framealpha=0.9)

    save_fig(fig, outdir, 's1_evasion_trajectory')


# ── S2: Outcome Distribution ────────────────────────────────────────────────

def plot_s2_outcome_distribution(datasets, outdir):
    """Grouped bar chart of differential_category proportions."""
    names = ordered_names(datasets)

    # Collect all categories that appear
    all_cats = set()
    cat_counts = {}  # {name: Counter}
    for name in names:
        rounds = datasets[name]['rounds']
        counts = Counter(r['differential_category'] for r in rounds)
        cat_counts[name] = counts
        all_cats.update(counts.keys())

    # Sort categories in a sensible order
    cat_order = ['Evasion', 'RealDetection', 'InstrumentationArtifact',
                 'MutationFailed', 'Flaky', 'StaticDetection',
                 'PayloadFailed', 'Ambiguous']
    categories = [c for c in cat_order if c in all_cats]

    x = np.arange(len(categories))
    width = 0.8 / len(names)

    fig, ax = plt.subplots(figsize=(9, 5))

    for i, name in enumerate(names):
        total = sum(cat_counts[name].values())
        proportions = [cat_counts[name].get(c, 0) / total for c in categories]
        raw_counts = [cat_counts[name].get(c, 0) for c in categories]
        color, _ = SELECTOR_STYLE[name]

        bars = ax.bar(x + i * width - 0.4 + width / 2, proportions, width * 0.9,
                      label=f'{name} (n={total})', color=color, alpha=0.85)

        # Annotate with absolute counts
        for bar, count in zip(bars, raw_counts):
            if count > 0:
                ax.text(bar.get_x() + bar.get_width() / 2, bar.get_height() + 0.01,
                        str(count), ha='center', va='bottom', fontsize=7)

    ax.set_xticks(x)
    ax.set_xticklabels(categories, rotation=25, ha='right')
    ax.set_ylabel('Proportion of Rounds')
    ax.set_title('Outcome Distribution by Selector Strategy')
    ax.legend(loc='upper right', framealpha=0.9)
    ax.set_ylim(0, ax.get_ylim()[1] * 1.12)

    save_fig(fig, outdir, 's2_outcome_distribution')


# ── S3: Summary Metrics ─────────────────────────────────────────────────────

def plot_s3_summary_metrics(datasets, outdir):
    """1x4 subplot grid of bar charts for key metrics."""
    names = ordered_names(datasets)

    # Compute metrics
    metrics = {name: {} for name in names}
    for name in names:
        rounds = datasets[name]['rounds']
        total = len(rounds)
        cats = Counter(r['differential_category'] for r in rounds)
        scores = [r.get('evasion_score') or 0 for r in rounds]
        tm = datasets[name].get('token_matrices') or []
        trustworthy = sum(1 for t in tm if t.get('trustworthy', False))

        metrics[name] = {
            'evasion_rate': cats.get('Evasion', 0) / total if total else 0,
            'mean_score': np.mean(scores) if scores else 0,
            'trustworthy_ratio': trustworthy / len(tm) if tm else 0,
            'mutfailed_rate': cats.get('MutationFailed', 0) / total if total else 0,
        }

    metric_defs = [
        ('evasion_rate', 'Evasion Rate', '{:.1%}'),
        ('mean_score', 'Mean Evasion Score', '{:.3f}'),
        ('trustworthy_ratio', 'Trustworthy Ratio', '{:.1%}'),
        ('mutfailed_rate', 'MutationFailed Rate', '{:.1%}'),
    ]

    fig, axes = plt.subplots(1, 4, figsize=(12, 4))

    for ax, (key, title, fmt) in zip(axes, metric_defs):
        values = [metrics[name][key] for name in names]
        colors = [SELECTOR_STYLE[name][0] for name in names]

        bars = ax.bar(range(len(names)), values, color=colors, alpha=0.85)
        ax.set_xticks(range(len(names)))
        ax.set_xticklabels(names, rotation=30, ha='right', fontsize=8)
        ax.set_title(title, fontsize=10)
        ax.set_ylim(0, max(max(values) * 1.3, 0.05))

        for bar, val in zip(bars, values):
            ax.text(bar.get_x() + bar.get_width() / 2, bar.get_height() + 0.005,
                    fmt.format(val), ha='center', va='bottom', fontsize=8)

    fig.suptitle('Summary Metrics by Selector Strategy', fontsize=12, y=1.02)
    fig.tight_layout()

    save_fig(fig, outdir, 's3_summary_metrics')


# ── S4: AST Mutation Heatmap ────────────────────────────────────────────────

def plot_s4_mutation_heatmap(datasets, outdir):
    """Heatmap of AST mutation frequency (normalized) per selector."""
    names = ordered_names(datasets)

    # Count AST mutations per selector, normalized by round count
    freq_raw = []    # shape: (n_selectors, n_mutations)
    freq_norm = []
    for name in names:
        rounds = datasets[name]['rounds']
        total = len(rounds)
        counts = Counter()
        for r in rounds:
            for m in get_ast_mutations(r):
                counts[m] += 1
        raw_row = [counts.get(m, 0) for m in ALL_AST_MUTATIONS]
        norm_row = [c / total if total else 0 for c in raw_row]
        freq_raw.append(raw_row)
        freq_norm.append(norm_row)

    raw_matrix = np.array(freq_raw).T    # (n_mutations, n_selectors)
    norm_matrix = np.array(freq_norm).T

    fig, ax = plt.subplots(figsize=(8, max(5, len(ALL_AST_MUTATIONS) * 0.45)))

    labels_short = [m.replace('ast.', '') for m in ALL_AST_MUTATIONS]

    try:
        import seaborn as sns
        sns.heatmap(norm_matrix, annot=raw_matrix.astype(int), fmt='d',
                    cmap='YlOrRd', xticklabels=names, yticklabels=labels_short,
                    ax=ax, cbar_kws={'label': 'Frequency (normalized)'})
    except ImportError:
        im = ax.imshow(norm_matrix, cmap='YlOrRd', aspect='auto')
        ax.set_xticks(range(len(names)))
        ax.set_xticklabels(names)
        ax.set_yticks(range(len(ALL_AST_MUTATIONS)))
        ax.set_yticklabels(labels_short, fontsize=8)
        for i in range(len(ALL_AST_MUTATIONS)):
            for j in range(len(names)):
                ax.text(j, i, str(int(raw_matrix[i, j])),
                        ha='center', va='center', fontsize=8)
        fig.colorbar(im, ax=ax, label='Frequency (normalized)')

    ax.set_title('AST Mutation Selection Frequency by Selector')
    ax.set_xlabel('Selector Strategy')
    ax.set_ylabel('AST Mutation')

    save_fig(fig, outdir, 's4_mutation_heatmap')


# ── S5: Mutation Recipe Size ────────────────────────────────────────────────

def plot_s5_mutation_recipe_size(datasets, outdir):
    """Box plot with strip overlay of AST mutation count per round."""
    names = ordered_names(datasets)

    data_by_selector = []
    for name in names:
        rounds = datasets[name]['rounds']
        sizes = [len(get_ast_mutations(r)) for r in rounds]
        data_by_selector.append(sizes)

    fig, ax = plt.subplots(figsize=(7, 4.5))

    # Box plot
    bp = ax.boxplot(data_by_selector, labels=names, patch_artist=True,
                    widths=0.5, showmeans=True,
                    meanprops=dict(marker='D', markerfacecolor='white',
                                   markeredgecolor='black', markersize=5))

    for patch, name in zip(bp['boxes'], names):
        color, _ = SELECTOR_STYLE[name]
        patch.set_facecolor(color)
        patch.set_alpha(0.4)

    # Strip overlay (jittered)
    for i, (name, sizes) in enumerate(zip(names, data_by_selector)):
        color, marker = SELECTOR_STYLE[name]
        jitter = np.random.default_rng(42).uniform(-0.15, 0.15, len(sizes))
        ax.scatter(np.full(len(sizes), i + 1) + jitter, sizes,
                   color=color, alpha=0.4, s=15, zorder=3, marker=marker)

    ax.set_ylabel('AST Mutations per Round')
    ax.set_title('Mutation Recipe Size by Selector Strategy')
    ax.set_ylim(-0.5, max(max(s) for s in data_by_selector if s) + 1)

    # Annotate means
    for i, sizes in enumerate(data_by_selector):
        if sizes:
            mean_val = np.mean(sizes)
            ax.text(i + 1, ax.get_ylim()[1] * 0.95,
                    f'$\\mu$={mean_val:.1f}', ha='center', fontsize=8)

    save_fig(fig, outdir, 's5_mutation_recipe_size')


# ── S6: Evasion Score CDF ───────────────────────────────────────────────────

def plot_s6_evasion_cdf(datasets, outdir):
    """Empirical CDF of evasion scores per selector."""
    names = ordered_names(datasets)
    fig, ax = plt.subplots(figsize=(7, 4.5))

    for name in names:
        rounds = datasets[name]['rounds']
        scores = sorted([r.get('evasion_score') or 0 for r in rounds])
        n = len(scores)
        cdf = np.arange(1, n + 1) / n

        color, marker = SELECTOR_STYLE[name]
        frac_above = sum(1 for s in scores if s > 0.6) / n if n else 0

        ax.step(scores, cdf, where='post', color=color, linewidth=1.5,
                label=f'{name} ({frac_above:.0%} > 0.6)')

    # Threshold lines
    ax.axvline(x=0.4, color='#E53935', linestyle=':', linewidth=0.8,
               alpha=0.5, label='Detection boundary (0.4)')
    ax.axvline(x=0.6, color='#43A047', linestyle='--', linewidth=0.8,
               alpha=0.5, label='Evasion boundary (0.6)')

    ax.set_xlabel('Evasion Score')
    ax.set_ylabel('Cumulative Proportion')
    ax.set_title('Evasion Score CDF by Selector Strategy')
    ax.set_xlim(-0.05, 1.05)
    ax.set_ylim(-0.02, 1.05)
    ax.legend(loc='lower right', framealpha=0.9, fontsize=8)

    save_fig(fig, outdir, 's6_evasion_cdf')


# ── S7: Token Diversity ─────────────────────────────────────────────────────

def plot_s7_token_diversity(datasets, outdir):
    """Grouped bars of unique behavioral token counts by prefix category."""
    names = ordered_names(datasets)

    # Collect unique behavioral tokens per selector per prefix
    prefix_counts = {}  # {name: {prefix: count}}
    all_prefixes = set()

    for name in names:
        tm_list = datasets[name].get('token_matrices') or []
        unique_tokens = set()
        for tm in tm_list:
            unique_tokens.update(get_behavioral_tokens(tm))

        counts = Counter()
        for tok in unique_tokens:
            prefix = tok.split(':')[0]
            counts[prefix] += 1

        prefix_counts[name] = counts
        all_prefixes.update(counts.keys())

    # Sort prefixes by total count descending
    prefixes = sorted(all_prefixes,
                      key=lambda p: sum(prefix_counts[n].get(p, 0) for n in names),
                      reverse=True)

    if not prefixes:
        print("  Skipping S7: no behavioral tokens found.")
        return

    x = np.arange(len(prefixes))
    width = 0.8 / len(names)

    fig, ax = plt.subplots(figsize=(10, 5))

    for i, name in enumerate(names):
        values = [prefix_counts[name].get(p, 0) for p in prefixes]
        color, _ = SELECTOR_STYLE[name]
        bars = ax.bar(x + i * width - 0.4 + width / 2, values, width * 0.9,
                      label=name, color=color, alpha=0.85)

        for bar, val in zip(bars, values):
            if val > 0:
                ax.text(bar.get_x() + bar.get_width() / 2,
                        bar.get_height() + 0.3,
                        str(val), ha='center', va='bottom', fontsize=7)

    ax.set_xticks(x)
    ax.set_xticklabels(prefixes, rotation=30, ha='right')
    ax.set_ylabel('Unique Tokens')
    ax.set_title('Behavioral Token Diversity by Selector Strategy')
    ax.legend(loc='upper right', framealpha=0.9)

    save_fig(fig, outdir, 's7_token_diversity')


# ── S8: Summary Table ───────────────────────────────────────────────────────

def generate_s8_summary_table(datasets, outdir):
    """Generate LaTeX booktabs table + CSV summary."""
    names = ordered_names(datasets)

    rows = []
    for name in names:
        rounds = datasets[name]['rounds']
        total = len(rounds)
        cats = Counter(r['differential_category'] for r in rounds)
        scores = [r.get('evasion_score') or 0 for r in rounds]
        tm_list = datasets[name].get('token_matrices') or []
        trustworthy = sum(1 for t in tm_list if t.get('trustworthy', False))

        # Unique AST mutations used
        ast_used = set()
        for r in rounds:
            ast_used.update(get_ast_mutations(r))

        # Unique behavioral tokens
        behavioral_tokens = set()
        for tm in tm_list:
            behavioral_tokens.update(get_behavioral_tokens(tm))

        rows.append({
            'name': name,
            'total_rounds': total,
            'evasion_count': cats.get('Evasion', 0),
            'evasion_rate': cats.get('Evasion', 0) / total if total else 0,
            'mean_score': np.mean(scores) if scores else 0,
            'max_score': max(scores) if scores else 0,
            'real_detection': cats.get('RealDetection', 0),
            'mutfailed_rate': cats.get('MutationFailed', 0) / total if total else 0,
            'trustworthy_ratio': trustworthy / len(tm_list) if tm_list else 0,
            'unique_ast': len(ast_used),
            'unique_behavioral': len(behavioral_tokens),
        })

    # ── LaTeX ──
    tex_path = os.path.join(outdir, 's8_selector_comparison_table.tex')
    with open(tex_path, 'w') as f:
        f.write('\\begin{table}[htbp]\n')
        f.write('\\centering\n')
        f.write('\\caption{Selector Strategy Comparison Summary}\n')
        f.write('\\label{tab:selector-comparison}\n')
        f.write('\\begin{tabular}{l' + 'r' * len(names) + '}\n')
        f.write('\\toprule\n')
        f.write('Metric & ' + ' & '.join(names) + ' \\\\\n')
        f.write('\\midrule\n')

        metric_labels = [
            ('total_rounds', 'Total rounds', '{}'),
            ('evasion_count', 'Evasion count', '{}'),
            ('evasion_rate', 'Evasion rate', '{:.1\\%}'),
            ('mean_score', 'Mean evasion score', '{:.3f}'),
            ('max_score', 'Max evasion score', '{:.3f}'),
            ('real_detection', 'RealDetection count', '{}'),
            ('mutfailed_rate', 'MutationFailed rate', '{:.1\\%}'),
            ('trustworthy_ratio', 'Trustworthy ratio', '{:.1\\%}'),
            ('unique_ast', 'Unique AST mutations', '{}'),
            ('unique_behavioral', 'Unique behavioral tokens', '{}'),
        ]

        for key, label, fmt in metric_labels:
            vals = []
            for row in rows:
                v = row[key]
                if '\\%' in fmt:
                    vals.append(f'{v * 100:.1f}\\%')
                elif 'f}' in fmt:
                    vals.append(f'{v:.3f}')
                else:
                    vals.append(str(v))
            f.write(f'{label} & ' + ' & '.join(vals) + ' \\\\\n')

        f.write('\\bottomrule\n')
        f.write('\\end{tabular}\n')
        f.write('\\end{table}\n')

    print(f"  Saved s8_selector_comparison_table.tex")

    # ── CSV ──
    csv_path = os.path.join(outdir, 's8_selector_comparison_table.csv')
    with open(csv_path, 'w') as f:
        header = ['Metric'] + names
        f.write(','.join(header) + '\n')

        for key, label, fmt in metric_labels:
            vals = []
            for row in rows:
                v = row[key]
                if '\\%' in fmt:
                    vals.append(f'{v * 100:.1f}%')
                elif 'f}' in fmt:
                    vals.append(f'{v:.3f}')
                else:
                    vals.append(str(v))
            f.write(','.join([label] + vals) + '\n')

    print(f"  Saved s8_selector_comparison_table.csv")


# ── Figure Registry ─────────────────────────────────────────────────────────

FIGURE_REGISTRY = {
    'S1': ('Evasion Score Trajectory', plot_s1_evasion_trajectory),
    'S2': ('Outcome Distribution', plot_s2_outcome_distribution),
    'S3': ('Summary Metrics', plot_s3_summary_metrics),
    'S4': ('AST Mutation Heatmap', plot_s4_mutation_heatmap),
    'S5': ('Mutation Recipe Size', plot_s5_mutation_recipe_size),
    'S6': ('Evasion Score CDF', plot_s6_evasion_cdf),
    'S7': ('Token Diversity', plot_s7_token_diversity),
    'S8': ('Summary Table', generate_s8_summary_table),
}


# ── Main ─────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(
        description='Generate selector comparison figures from EvalDataset JSONs')

    parser.add_argument('--coverage', type=str, help='Path to coverage_eval.json')
    parser.add_argument('--ga', type=str, help='Path to ga_eval.json (Fuzzer/GA)')
    parser.add_argument('--random', type=str, help='Path to random_eval.json')
    parser.add_argument('--token', type=str, help='Path to token_eval.json')
    parser.add_argument('--outdir', type=str, default='evaluation/figures/compare',
                        help='Output directory for figures (default: evaluation/figures/compare)')
    parser.add_argument('--window', type=int, default=5,
                        help='Rolling mean window size (default: 5)')
    parser.add_argument('--only', type=str, default=None,
                        help='Comma-separated list of figures to generate (e.g., S1,S4)')

    args = parser.parse_args()

    # Load datasets
    datasets = load_datasets(args)
    names = ordered_names(datasets)
    print(f"Loaded {len(datasets)} datasets: {', '.join(names)}")
    for name in names:
        n = len(datasets[name]['rounds'])
        print(f"  {name}: {n} rounds")

    # Create output directory
    os.makedirs(args.outdir, exist_ok=True)

    # Determine which figures to generate
    if args.only:
        selected = [s.strip().upper() for s in args.only.split(',')]
    else:
        selected = list(FIGURE_REGISTRY.keys())

    # Generate figures
    for fig_id in selected:
        if fig_id not in FIGURE_REGISTRY:
            print(f"Warning: unknown figure '{fig_id}', skipping.")
            continue

        label, func = FIGURE_REGISTRY[fig_id]
        print(f"\n[{fig_id}] {label}")

        if fig_id == 'S1':
            func(datasets, args.outdir, window=args.window)
        elif fig_id == 'S8':
            func(datasets, args.outdir)
        else:
            func(datasets, args.outdir)

    print(f"\nDone. Figures saved to {args.outdir}/")


if __name__ == '__main__':
    main()

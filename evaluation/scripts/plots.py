#!/usr/bin/env python3
"""
Component-Level Evaluation Plots

Generates thesis-quality figures from the component_eval_report.json output.

Usage:
    python evaluation/scripts/plots.py [--input component_eval_report.json] [--outdir figures/]

Requirements:
    pip install matplotlib seaborn numpy

Generates figures for:
    C1: Token sensitivity heatmap
    C3: Token coverage stacked bar + presence heatmap
    C4: Scoring convergence curves
    C5: Counterfactual forest plot + volcano plot
    B2: Classifier confusion matrix + Sankey
    B3: Telemetry coverage by verdict
"""

import argparse
import json
import os
import sys
from pathlib import Path

import matplotlib
matplotlib.use('Agg')  # Non-interactive backend
import matplotlib.pyplot as plt
import matplotlib.patches as mpatches
import numpy as np

# Thesis-quality defaults
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


def load_report(path):
    """Load component_eval_report.json and index by metric_id."""
    with open(path) as f:
        metrics = json.load(f)
    return {m['metric_id']: m for m in metrics}


def find_metric(metrics, prefix):
    """Find first metric matching prefix."""
    for mid, m in metrics.items():
        if mid.startswith(prefix):
            return m
    return None


# ── C1: Token Sensitivity Heatmap ─────────────────────────────────────────

def plot_c1_heatmap(metrics, outdir):
    """C1: 5×5 heatmap of actionable token count vs (lift, confidence)."""
    m = find_metric(metrics, 'component.c1.token_sensitivity.heatmap')
    if not m:
        print("  Skipping C1 heatmap: metric not found")
        return

    d = m['details']
    lifts = d['lift_thresholds']
    confs = d['min_confidences']
    heatmap = d['heatmap']

    # Extract actionable counts into 2D array
    data = np.zeros((len(lifts), len(confs)))
    for i, row in enumerate(heatmap):
        for j, cell in enumerate(row):
            data[i, j] = cell['actionable']

    fig, ax = plt.subplots(figsize=(6, 4.5))
    im = ax.imshow(data, cmap='YlOrRd', aspect='auto', origin='lower')
    ax.set_xticks(range(len(confs)))
    ax.set_xticklabels([f'{c:.1f}' for c in confs])
    ax.set_yticks(range(len(lifts)))
    ax.set_yticklabels([f'{l:.1f}' for l in lifts])
    ax.set_xlabel('Minimum Confidence')
    ax.set_ylabel('Lift Threshold')
    ax.set_title('C1: Actionable Token Count by Scoring Parameters')

    # Annotate cells
    for i in range(len(lifts)):
        for j in range(len(confs)):
            val = int(data[i, j])
            color = 'white' if data[i, j] > data.max() * 0.6 else 'black'
            ax.text(j, i, str(val), ha='center', va='center', color=color, fontsize=10)

    fig.colorbar(im, ax=ax, label='Actionable Tokens (avoid + seek)')
    fig.savefig(os.path.join(outdir, 'c1_sensitivity_heatmap.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'c1_sensitivity_heatmap.png'), bbox_inches='tight')
    plt.close(fig)
    print("  C1: c1_sensitivity_heatmap.pdf")


# ── C3: Token Coverage ────────────────────────────────────────────────────

def plot_c3_coverage(metrics, outdir):
    """C3: Stacked bar of token categories per round + coverage table."""
    m = find_metric(metrics, 'component.c3.token_coverage.category_table')
    if not m:
        print("  Skipping C3 coverage: metric not found")
        return

    d = m['details']
    table = d['coverage_table']

    # Bar chart of unique tokens per category
    categories = [row['category'] for row in table]
    unique = [row['unique_tokens'] for row in table]
    occurrences = [row['total_occurrences'] for row in table]

    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(12, 5))

    # Left: unique tokens per category
    colors = plt.cm.Set3(np.linspace(0, 1, len(categories)))
    bars = ax1.barh(categories, unique, color=colors)
    ax1.set_xlabel('Unique Tokens')
    ax1.set_title('C3: Unique Tokens per Category')
    for bar, val in zip(bars, unique):
        ax1.text(bar.get_width() + 0.3, bar.get_y() + bar.get_height()/2,
                 str(val), va='center', fontsize=9)

    # Right: occurrence proportion pie
    nonzero = [(c, o) for c, o in zip(categories, occurrences) if o > 0]
    if nonzero:
        labels, values = zip(*nonzero)
        ax2.pie(values, labels=labels, autopct='%1.1f%%', startangle=90,
                colors=plt.cm.Set3(np.linspace(0, 1, len(nonzero))))
        ax2.set_title('C3: Token Occurrence Proportions')

    fig.tight_layout()
    fig.savefig(os.path.join(outdir, 'c3_token_coverage.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'c3_token_coverage.png'), bbox_inches='tight')
    plt.close(fig)
    print("  C3: c3_token_coverage.pdf")


def plot_c3_heatmap(metrics, outdir):
    """C3: Token presence heatmap (top-20 tokens × rounds)."""
    m = find_metric(metrics, 'component.c3.token_coverage.presence_heatmap')
    if not m:
        print("  Skipping C3 heatmap: metric not found")
        return

    d = m['details']
    labels = d['token_labels']
    heatmap = np.array(d['heatmap'], dtype=float)
    rounds = d['round_numbers']

    if heatmap.size == 0:
        return

    fig, ax = plt.subplots(figsize=(12, 6))
    im = ax.imshow(heatmap.T, cmap='Blues', aspect='auto', interpolation='nearest')
    ax.set_xlabel('Round')
    ax.set_ylabel('Token')
    ax.set_xticks(range(len(rounds)))
    ax.set_xticklabels(rounds, fontsize=7)
    ax.set_yticks(range(len(labels)))

    # Truncate long labels
    short_labels = [l[:40] for l in labels]
    ax.set_yticklabels(short_labels, fontsize=7)
    ax.set_title('C3: Token Presence Heatmap (Top-20 Tokens)')

    fig.colorbar(im, ax=ax, label='Present', ticks=[0, 1])
    fig.savefig(os.path.join(outdir, 'c3_presence_heatmap.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'c3_presence_heatmap.png'), bbox_inches='tight')
    plt.close(fig)
    print("  C3: c3_presence_heatmap.pdf")


# ── C4: Scoring Convergence ───────────────────────────────────────────────

def plot_c4_convergence(metrics, outdir):
    """C4: Top-5 overlap and actionable count vs rounds."""
    m = find_metric(metrics, 'component.c4.scoring_convergence.top5_overlap')
    if not m:
        print("  Skipping C4 convergence: metric not found")
        return

    d = m['details']
    curve = d['convergence_curve']

    rounds = [pt['rounds'] for pt in curve]
    overlap = [pt['top5_overlap_frac'] for pt in curve]
    actionable = [pt['actionable_count'] for pt in curve]

    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(12, 4.5))

    # Left: top-5 overlap fraction
    ax1.plot(rounds, overlap, 'o-', color='#2196F3', linewidth=2, markersize=8)
    ax1.set_xlabel('Rounds Included')
    ax1.set_ylabel('Top-5 Overlap with Final')
    ax1.set_title('C4: Token Ranking Convergence')
    ax1.set_ylim(-0.05, 1.05)
    ax1.axhline(y=0.8, color='gray', linestyle='--', alpha=0.5, label='80% threshold')
    ax1.legend()

    # Right: actionable token count
    ax2.plot(rounds, actionable, 's-', color='#FF9800', linewidth=2, markersize=8)
    ax2.set_xlabel('Rounds Included')
    ax2.set_ylabel('Actionable Tokens (avoid + seek)')
    ax2.set_title('C4: Actionable Token Count over Rounds')

    fig.tight_layout()
    fig.savefig(os.path.join(outdir, 'c4_scoring_convergence.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'c4_scoring_convergence.png'), bbox_inches='tight')
    plt.close(fig)
    print("  C4: c4_scoring_convergence.pdf")


# ── C5: Counterfactual Validation ─────────────────────────────────────────

def plot_c5_forest(metrics, outdir):
    """C5: Forest plot of detection rate deltas with 95% CI."""
    m = find_metric(metrics, 'component.c5.counterfactual.forest_plot')
    if not m:
        print("  Skipping C5 forest: metric not found")
        return

    d = m['details']
    results = d['token_results']

    if not results:
        return

    # Filter to tokens with sufficient observations (n >= 3 on each side)
    filtered = [r for r in results
                if r['n_with'] >= 3 and r['n_without'] >= 3]

    if not filtered:
        filtered = results[:15]  # Fallback: show top 15

    # Sort by delta
    filtered.sort(key=lambda r: r['delta'])

    tokens = [r['token'][:35] for r in filtered]
    deltas = [r['delta'] for r in filtered]
    ci_low = [r['delta_ci_low'] for r in filtered]
    ci_high = [r['delta_ci_high'] for r in filtered]
    significant = [r.get('significant_005', False) for r in filtered]

    fig, ax = plt.subplots(figsize=(8, max(4, len(filtered) * 0.35)))

    y_pos = range(len(tokens))
    colors = ['#d32f2f' if s else '#757575' for s in significant]

    ax.barh(y_pos, deltas, color=colors, alpha=0.7, height=0.6)

    # Error bars for CI
    for i in range(len(filtered)):
        ax.plot([ci_low[i], ci_high[i]], [i, i], color='black', linewidth=1)

    ax.axvline(x=0, color='black', linewidth=0.8)
    ax.set_yticks(y_pos)
    ax.set_yticklabels(tokens, fontsize=8)
    ax.set_xlabel('Detection Rate Delta (with − without token)')
    ax.set_title('C5: Counterfactual Token Attribution (Forest Plot)')

    # Legend
    sig_patch = mpatches.Patch(color='#d32f2f', alpha=0.7, label='Significant (p < 0.05)')
    ns_patch = mpatches.Patch(color='#757575', alpha=0.7, label='Not significant')
    ax.legend(handles=[sig_patch, ns_patch], loc='lower right')

    fig.savefig(os.path.join(outdir, 'c5_forest_plot.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'c5_forest_plot.png'), bbox_inches='tight')
    plt.close(fig)
    print("  C5: c5_forest_plot.pdf")


def plot_c5_volcano(metrics, outdir):
    """C5: Volcano plot (-log10(p) vs delta)."""
    m = find_metric(metrics, 'component.c5.counterfactual.forest_plot')
    if not m:
        print("  Skipping C5 volcano: metric not found")
        return

    d = m['details']
    results = d['token_results']

    if not results:
        return

    deltas = [r['delta'] for r in results]
    neg_log_p = [r['neg_log10_p'] for r in results]
    significant = [r.get('significant_005', False) for r in results]

    fig, ax = plt.subplots(figsize=(7, 5))

    for i in range(len(results)):
        color = '#d32f2f' if significant[i] else '#9e9e9e'
        size = 60 if significant[i] else 30
        ax.scatter(deltas[i], neg_log_p[i], c=color, s=size, alpha=0.7, edgecolors='none')

    # Significance threshold line
    bonf_threshold = -np.log10(0.05 / len(results)) if len(results) > 0 else 1.3
    ax.axhline(y=bonf_threshold, color='red', linestyle='--', alpha=0.5,
               label=f'Bonferroni α=0.05 (p={0.05/len(results):.4f})')
    ax.axvline(x=0, color='black', linewidth=0.5)

    ax.set_xlabel('Detection Rate Delta')
    ax.set_ylabel('−log₁₀(p-value)')
    ax.set_title('C5: Volcano Plot — Token Effect Size vs Significance')
    ax.legend()

    fig.savefig(os.path.join(outdir, 'c5_volcano_plot.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'c5_volcano_plot.png'), bbox_inches='tight')
    plt.close(fig)
    print("  C5: c5_volcano_plot.pdf")


# ── B2: Classifier Analysis ──────────────────────────────────────────────

def plot_b2_confusion(metrics, outdir):
    """B2: Confusion matrix (verdict → category)."""
    m = find_metric(metrics, 'component.b2.classifier_analysis.confusion_matrix')
    if not m:
        print("  Skipping B2 confusion: metric not found")
        return

    d = m['details']
    verdicts = d['verdicts']
    categories = d['categories']
    matrix = np.array(d['matrix'])

    fig, ax = plt.subplots(figsize=(8, 5))
    im = ax.imshow(matrix, cmap='Blues', aspect='auto')

    ax.set_xticks(range(len(categories)))
    ax.set_xticklabels(categories, rotation=45, ha='right', fontsize=8)
    ax.set_yticks(range(len(verdicts)))
    ax.set_yticklabels(verdicts, fontsize=9)
    ax.set_xlabel('Differential Category')
    ax.set_ylabel('Detection Verdict')
    ax.set_title('B2: Verdict → Category Confusion Matrix')

    # Annotate cells
    for i in range(len(verdicts)):
        for j in range(len(categories)):
            val = int(matrix[i, j])
            if val > 0:
                color = 'white' if matrix[i, j] > matrix.max() * 0.5 else 'black'
                ax.text(j, i, str(val), ha='center', va='center', color=color, fontsize=10)

    fig.colorbar(im, ax=ax, label='Count')
    fig.savefig(os.path.join(outdir, 'b2_confusion_matrix.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'b2_confusion_matrix.png'), bbox_inches='tight')
    plt.close(fig)
    print("  B2: b2_confusion_matrix.pdf")


# ── B3: Telemetry Completeness ────────────────────────────────────────────

def plot_b3_coverage(metrics, outdir):
    """B3: Coverage distribution by verdict (box plot)."""
    m = find_metric(metrics, 'component.b3.telemetry_completeness.coverage_by_verdict')
    if not m:
        print("  Skipping B3 coverage: metric not found")
        return

    d = m['details']
    box_data = d['box_plot_data']

    if not box_data:
        return

    fig, ax = plt.subplots(figsize=(8, 5))

    labels = [b['category'] for b in box_data]
    data = [b['values'] for b in box_data]

    bp = ax.boxplot(data, labels=labels, patch_artist=True, showmeans=True)

    colors = plt.cm.Set2(np.linspace(0, 1, len(labels)))
    for patch, color in zip(bp['boxes'], colors):
        patch.set_facecolor(color)
        patch.set_alpha(0.7)

    ax.set_xlabel('Differential Category')
    ax.set_ylabel('Coverage Percent')
    ax.set_title('B3: Telemetry Coverage by Outcome Category')
    plt.xticks(rotation=30, ha='right')

    fig.savefig(os.path.join(outdir, 'b3_coverage_boxplot.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'b3_coverage_boxplot.png'), bbox_inches='tight')
    plt.close(fig)
    print("  B3: b3_coverage_boxplot.pdf")


def plot_b3_histogram(metrics, outdir):
    """B3: Coverage percent histogram."""
    m = find_metric(metrics, 'component.b3.telemetry_completeness.coverage_histogram')
    if not m:
        print("  Skipping B3 histogram: metric not found")
        return

    d = m['details']
    values = d['all_values']

    fig, ax = plt.subplots(figsize=(7, 4))
    ax.hist(values, bins=20, color='#42A5F5', edgecolor='white', alpha=0.8)
    ax.set_xlabel('Coverage Percent')
    ax.set_ylabel('Frequency')
    ax.set_title('B3: Coverage Distribution')
    ax.axvline(x=np.mean(values), color='red', linestyle='--',
               label=f'Mean = {np.mean(values):.2f}')
    ax.legend()

    fig.savefig(os.path.join(outdir, 'b3_coverage_histogram.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'b3_coverage_histogram.png'), bbox_inches='tight')
    plt.close(fig)
    print("  B3: b3_coverage_histogram.pdf")


# ── Summary Table ─────────────────────────────────────────────────────────

def generate_summary_table(metrics, outdir):
    """Generate a LaTeX summary table of all component metrics."""
    rows = []
    for mid, m in sorted(metrics.items()):
        rows.append({
            'id': mid.replace('component.', ''),
            'label': m['label'][:60],
            'value': m['value'],
            'n': m['n'],
        })

    # Write LaTeX table
    latex_path = os.path.join(outdir, 'component_metrics_table.tex')
    with open(latex_path, 'w') as f:
        f.write('\\begin{table}[htbp]\n')
        f.write('\\centering\n')
        f.write('\\caption{Component-Level Evaluation Metrics}\n')
        f.write('\\label{tab:component-metrics}\n')
        f.write('\\begin{tabular}{llrr}\n')
        f.write('\\toprule\n')
        f.write('Metric ID & Label & Value & $n$ \\\\\n')
        f.write('\\midrule\n')

        current_prefix = ''
        for row in rows:
            prefix = row['id'].split('.')[0]
            if prefix != current_prefix:
                if current_prefix:
                    f.write('\\midrule\n')
                current_prefix = prefix

            label = row['label'].replace('&', '\\&').replace('_', '\\_')
            mid = row['id'].replace('_', '\\_')
            f.write(f"\\texttt{{{mid}}} & {label} & {row['value']:.4f} & {row['n']} \\\\\n")

        f.write('\\bottomrule\n')
        f.write('\\end{tabular}\n')
        f.write('\\end{table}\n')

    print(f"  Summary: {latex_path}")


# ── Main ──────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description='Generate evaluation figures')
    parser.add_argument('--input', default='component_eval_report.json',
                        help='Input JSON report from component-eval')
    parser.add_argument('--outdir', default='figures',
                        help='Output directory for figures')
    args = parser.parse_args()

    if not os.path.exists(args.input):
        print(f"Error: {args.input} not found")
        print("Run: cargo run -p evaluation --features full --bin component-eval")
        sys.exit(1)

    os.makedirs(args.outdir, exist_ok=True)
    metrics = load_report(args.input)
    print(f"Loaded {len(metrics)} metrics from {args.input}")
    print(f"Output directory: {args.outdir}\n")

    # Generate all plots
    print("Generating plots:")
    plot_c1_heatmap(metrics, args.outdir)
    plot_c3_coverage(metrics, args.outdir)
    plot_c3_heatmap(metrics, args.outdir)
    plot_c4_convergence(metrics, args.outdir)
    plot_c5_forest(metrics, args.outdir)
    plot_c5_volcano(metrics, args.outdir)
    plot_b2_confusion(metrics, args.outdir)
    plot_b3_coverage(metrics, args.outdir)
    plot_b3_histogram(metrics, args.outdir)

    print("\nGenerating tables:")
    generate_summary_table(metrics, args.outdir)

    print(f"\nDone. {len(os.listdir(args.outdir))} files in {args.outdir}/")


if __name__ == '__main__':
    main()

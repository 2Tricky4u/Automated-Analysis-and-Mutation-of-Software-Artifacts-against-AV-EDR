#!/usr/bin/env python3
"""
Component-Level and Infrastructure-Level Evaluation Plots

Generates thesis-quality figures from evaluation report JSON files.

Usage:
    python evaluation/scripts/plots.py [--input component_eval_report.json] [--outdir figures/]
    python evaluation/scripts/plots.py --infra [--input infra_eval_report.json] [--outdir figures/]

Requirements:
    pip install matplotlib seaborn numpy

Generates figures for:
    Component: C1, C3, C4, C5, B2, B3
    Infrastructure: I1-I13 (with --infra flag)
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
    ax.set_title('C3: Token Presence Heatmap (20 Rarest Tokens)')

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


# ── I1: Payload Encoding ─────────────────────────────────────────────────

def plot_i1_entropy(metrics, outdir):
    """I1: Grouped bar chart of entropy by encoding type x payload size."""
    m = find_metric(metrics, 'infra.i1.payload_encoding.entropy_comparison')
    if not m:
        print("  Skipping I1 entropy: metric not found")
        return

    d = m['details']
    type_data = d['type_entropies']

    fig, ax = plt.subplots(figsize=(8, 5))
    n_types = len(type_data)
    width = 0.8 / max(n_types, 1)

    for idx, entry in enumerate(sorted(type_data, key=lambda x: x['encoding_type'])):
        sizes = [pt['payload_size'] for pt in entry['by_size']]
        entropies = [pt['entropy'] for pt in entry['by_size']]
        x = np.arange(len(sizes))
        offset = (idx - n_types / 2 + 0.5) * width
        ax.bar(x + offset, entropies, width, label=entry['encoding_type'], alpha=0.85)

    ax.set_xticks(range(len(sizes)))
    ax.set_xticklabels([str(s) for s in sizes])
    ax.set_xlabel('Payload Size (bytes)')
    ax.set_ylabel('Shannon Entropy (bits/byte)')
    ax.set_title('I1: Encoded Payload Entropy by Encoding Type')
    ax.legend()
    ax.set_ylim(0, 9)

    fig.savefig(os.path.join(outdir, 'i1_encoding_entropy.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i1_encoding_entropy.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I1: i1_encoding_entropy.pdf")


def plot_i1_expansion(metrics, outdir):
    """I1: Grouped bar chart of size expansion ratio per encoding type."""
    m = find_metric(metrics, 'infra.i1.payload_encoding.size_expansion')
    if not m:
        print("  Skipping I1 expansion: metric not found")
        return

    d = m['details']
    by_type = d['by_type']

    types = [e['encoding_type'] for e in by_type]
    means = [e['mean_expansion'] for e in by_type]
    maxes = [e['max_expansion'] for e in by_type]

    fig, ax = plt.subplots(figsize=(7, 4))
    x = np.arange(len(types))
    ax.bar(x - 0.15, means, 0.3, label='Mean', color='#2196F3', alpha=0.8)
    ax.bar(x + 0.15, maxes, 0.3, label='Max', color='#FF9800', alpha=0.8)
    ax.set_xticks(x)
    ax.set_xticklabels(types)
    ax.set_xlabel('Encoding Type')
    ax.set_ylabel('Size Expansion Ratio (encoded/original)')
    ax.set_title('I1: Payload Size Expansion by Encoding Type')
    ax.axhline(y=1.0, color='gray', linestyle='--', alpha=0.5)
    ax.legend()

    fig.savefig(os.path.join(outdir, 'i1_encoding_expansion.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i1_encoding_expansion.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I1: i1_encoding_expansion.pdf")


# ── I2: AST Mutation Impact ─────────────────────────────────────────────

def plot_i2_impact(metrics, outdir):
    """I2: Horizontal bar chart of line delta per AST mutation."""
    m = find_metric(metrics, 'infra.i2.ast_mutation.line_impact')
    if not m:
        print("  Skipping I2 impact: metric not found")
        return

    d = m['details']
    mutations = d['mutations']

    labels = [mut['mutation_id'][:35] for mut in mutations]
    deltas = [mut['line_delta'] for mut in mutations]
    valid = [True] * len(mutations)  # Check parse_validity metric separately

    fig, ax = plt.subplots(figsize=(9, max(4, len(labels) * 0.4)))
    colors = ['#4CAF50' if d >= 0 else '#f44336' for d in deltas]
    ax.barh(range(len(labels)), deltas, color=colors, alpha=0.8)
    ax.set_yticks(range(len(labels)))
    ax.set_yticklabels(labels, fontsize=8)
    ax.set_xlabel('Line Delta (output - input)')
    ax.set_title('I2: AST Mutation Source Impact')
    ax.axvline(x=0, color='black', linewidth=0.8)

    for i, d in enumerate(deltas):
        ax.text(d + (1 if d >= 0 else -1), i, str(d), va='center', fontsize=8)

    fig.savefig(os.path.join(outdir, 'i2_ast_mutation_impact.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i2_ast_mutation_impact.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I2: i2_ast_mutation_impact.pdf")


# ── I3: IR Mutation Analysis ─────────────────────────────────────────────

def plot_i3_ir_analysis(metrics, outdir):
    """I3: Grouped bar of insertions by mutation + O2 survival table."""
    m = find_metric(metrics, 'infra.i3.ir_mutation.insertion_effectiveness')
    if not m:
        print("  Skipping I3 analysis: metric not found")
        return

    d = m['details']
    by_mut = d['by_mutation']

    labels = [e['mutation_id'][:40] for e in by_mut]
    insertions = [e['insertions'] for e in by_mut]
    bloat = [e['line_bloat'] for e in by_mut]

    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(12, 5))

    # Left: insertions
    ax1.barh(range(len(labels)), insertions, color='#2196F3', alpha=0.8)
    ax1.set_yticks(range(len(labels)))
    ax1.set_yticklabels(labels, fontsize=8)
    ax1.set_xlabel('Lines Inserted')
    ax1.set_title('I3: IR Mutation Insertions')

    # Right: line bloat ratio
    ax2.barh(range(len(labels)), bloat, color='#FF9800', alpha=0.8)
    ax2.set_yticks(range(len(labels)))
    ax2.set_yticklabels(labels, fontsize=8)
    ax2.set_xlabel('Output/Input Line Ratio')
    ax2.set_title('I3: IR Line Bloat')
    ax2.axvline(x=1.0, color='gray', linestyle='--', alpha=0.5)

    fig.tight_layout()
    fig.savefig(os.path.join(outdir, 'i3_ir_mutation_analysis.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i3_ir_mutation_analysis.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I3: i3_ir_mutation_analysis.pdf")


# ── I4: Binary Mutation Heatmap ──────────────────────────────────────────

def plot_i4_pe_heatmap(metrics, outdir):
    """I4: Heatmap of 9 mutations x 4 features."""
    m = find_metric(metrics, 'infra.i4.binary_mutation.feature_heatmap')
    if not m:
        print("  Skipping I4 heatmap: metric not found")
        return

    d = m['details']
    heatmap_data = d['heatmap']
    features = d['features']

    labels = [e['mutation_id'][:30] for e in heatmap_data]
    data = np.zeros((len(labels), len(features)))
    for i, entry in enumerate(heatmap_data):
        for j, feat in enumerate(features):
            val = entry.get(feat, 0)
            data[i, j] = val

    # Normalize each column to [0,1] for display
    for j in range(data.shape[1]):
        col_max = np.abs(data[:, j]).max()
        if col_max > 0:
            data[:, j] = data[:, j] / col_max

    fig, ax = plt.subplots(figsize=(8, max(4, len(labels) * 0.4)))
    im = ax.imshow(data, cmap='RdYlBu_r', aspect='auto', vmin=-1, vmax=1)
    ax.set_xticks(range(len(features)))
    ax.set_xticklabels([f.replace('_', ' ') for f in features], rotation=45, ha='right')
    ax.set_yticks(range(len(labels)))
    ax.set_yticklabels(labels, fontsize=8)
    ax.set_title('I4: Binary Mutation Feature Impact (normalized)')
    fig.colorbar(im, ax=ax, label='Normalized Impact')

    fig.savefig(os.path.join(outdir, 'i4_binary_mutation_heatmap.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i4_binary_mutation_heatmap.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I4: i4_binary_mutation_heatmap.pdf")


# ── I5: Template Assembly ────────────────────────────────────────────────

def plot_i5_assembly(metrics, outdir):
    """I5: Assembly time histogram + marker resolution rate."""
    m = find_metric(metrics, 'infra.i5.template_assembly.latency')
    if not m:
        print("  Skipping I5 assembly: metric not found")
        return

    d = m['details']
    times = d['all_times_us']

    fig, ax = plt.subplots(figsize=(7, 4))
    ax.hist(times, bins=20, color='#4CAF50', edgecolor='white', alpha=0.8)
    ax.set_xlabel('Assembly Time (us)')
    ax.set_ylabel('Frequency')
    ax.set_title(f'I5: Template Assembly Time Distribution (n={len(times)})')
    ax.axvline(x=d['mean_us'], color='red', linestyle='--',
               label=f'Mean = {d["mean_us"]:.0f} us')
    ax.legend()

    fig.savefig(os.path.join(outdir, 'i5_template_assembly.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i5_template_assembly.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I5: i5_template_assembly.pdf")


# ── I6: Instrumentation Overhead ─────────────────────────────────────────

def plot_i6_overhead(metrics, outdir):
    """I6: Grouped bar of size ratio per carrier module."""
    m = find_metric(metrics, 'infra.i6.instrumentation.size_overhead')
    if not m:
        print("  Skipping I6 overhead: metric not found")
        return

    d = m['details']
    per_carrier = d['per_carrier']

    carriers = [e['carrier'] for e in per_carrier]
    baseline = [e['baseline_size'] for e in per_carrier]
    instrumented = [e['instrumented_size'] for e in per_carrier]

    fig, ax = plt.subplots(figsize=(8, 5))
    x = np.arange(len(carriers))
    ax.bar(x - 0.2, baseline, 0.35, label='Baseline', color='#2196F3', alpha=0.8)
    ax.bar(x + 0.2, instrumented, 0.35, label='Instrumented', color='#FF9800', alpha=0.8)
    ax.set_xticks(x)
    ax.set_xticklabels(carriers)
    ax.set_xlabel('Carrier Module')
    ax.set_ylabel('PE Size (bytes)')
    ax.set_title('I6: Instrumentation Size Overhead by Carrier')
    ax.legend()

    # Add ratio labels
    for i, entry in enumerate(per_carrier):
        ratio = entry['size_ratio']
        ax.text(i, max(baseline[i], instrumented[i]) * 1.02,
                f'{ratio:.2f}x', ha='center', fontsize=9)

    fig.savefig(os.path.join(outdir, 'i6_instrumentation_overhead.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i6_instrumentation_overhead.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I6: i6_instrumentation_overhead.pdf")


# ── I7: Token Extraction ─────────────────────────────────────────────────

def plot_i7_tokens(metrics, outdir):
    """I7: Stacked bar of tokens per category + latency histogram."""
    m_cat = find_metric(metrics, 'infra.i7.token_extraction.category_coverage')
    m_lat = find_metric(metrics, 'infra.i7.token_extraction.latency_distribution')
    if not m_cat and not m_lat:
        print("  Skipping I7 tokens: metrics not found")
        return

    fig, axes = plt.subplots(1, 2, figsize=(12, 5))

    # Left: category coverage stacked bar
    if m_cat:
        table = m_cat['details']['category_table']
        cats = [e['category'] for e in table]
        counts = [e['total_tokens'] for e in table]
        colors = plt.cm.Set3(np.linspace(0, 1, len(cats)))
        axes[0].barh(cats, counts, color=colors, alpha=0.85)
        axes[0].set_xlabel('Total Tokens Extracted')
        axes[0].set_title(f'I7: Token Categories ({m_cat["details"]["active_categories"]}/{m_cat["details"]["total_categories"]} active)')

    # Right: latency histogram
    if m_lat:
        times = m_lat['details']['all_times_us']
        axes[1].hist(times, bins=15, color='#42A5F5', edgecolor='white', alpha=0.8)
        axes[1].set_xlabel('Extraction Time (us)')
        axes[1].set_ylabel('Frequency')
        axes[1].set_title('I7: Extraction Latency Distribution')
        mean_t = m_lat['details']['mean_us']
        axes[1].axvline(x=mean_t, color='red', linestyle='--',
                        label=f'Mean = {mean_t:.0f} us')
        axes[1].legend()

    fig.tight_layout()
    fig.savefig(os.path.join(outdir, 'i7_token_extraction.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i7_token_extraction.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I7: i7_token_extraction.pdf")


# ── I8: Scoring Validation ───────────────────────────────────────────────

def plot_i8_scoring(metrics, outdir):
    """I8: Table figure showing expected vs computed lift values."""
    m = find_metric(metrics, 'infra.i8.token_scoring.lift_accuracy')
    if not m:
        print("  Skipping I8 scoring: metric not found")
        return

    d = m['details']
    cases = d['test_cases']

    fig, ax = plt.subplots(figsize=(10, max(3, len(cases) * 0.5 + 1)))
    ax.axis('off')

    col_labels = ['Test Case', 'Rounds', 'Expected Lift', 'Computed Lift', 'Error', 'Pass']
    cell_text = []
    cell_colors = []

    for c in cases:
        passed = c['pass']
        row = [
            c['test_case'],
            str(c['input_rounds']),
            f"{c['expected_lift']:.4f}",
            f"{c['computed_lift']:.4f}",
            f"{c['lift_error']:.6f}",
            'PASS' if passed else 'FAIL',
        ]
        cell_text.append(row)
        bg = '#e8f5e9' if passed else '#ffebee'
        cell_colors.append([bg] * 6)

    table = ax.table(cellText=cell_text, colLabels=col_labels,
                     cellColours=cell_colors, loc='center', cellLoc='center')
    table.auto_set_font_size(False)
    table.set_fontsize(9)
    table.scale(1.0, 1.4)

    ax.set_title('I8: Token Scoring Validation Results', fontsize=12, pad=20)

    fig.savefig(os.path.join(outdir, 'i8_scoring_validation.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i8_scoring_validation.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I8: i8_scoring_validation.pdf")


# ── I9: Input Diversity Heatmap ──────────────────────────────────────────

def plot_i9_diversity(metrics, outdir):
    """I9: 10x10 pairwise mutation distance heatmap."""
    m = find_metric(metrics, 'infra.i9.input_diversity.pairwise_distance')
    if not m:
        print("  Skipping I9 diversity: metric not found")
        return

    d = m['details']
    hm = d.get('heatmap', {})
    labels = hm.get('labels', [])
    matrix = np.array(hm.get('matrix', []))

    if matrix.size == 0 or len(labels) == 0:
        print("  Skipping I9 diversity: no heatmap data")
        return

    fig, ax = plt.subplots(figsize=(8, 7))
    im = ax.imshow(matrix, cmap='YlOrRd', aspect='auto', vmin=0, vmax=1)

    short_labels = [l.split(':')[0].replace('ast.', '') for l in labels]
    ax.set_xticks(range(len(short_labels)))
    ax.set_xticklabels(short_labels, rotation=45, ha='right', fontsize=8)
    ax.set_yticks(range(len(short_labels)))
    ax.set_yticklabels(short_labels, fontsize=8)
    ax.set_title('I9: Pairwise Mutation Output Distance')

    # Annotate cells
    for i in range(len(labels)):
        for j in range(len(labels)):
            if i != j:
                val = matrix[i, j]
                color = 'white' if val > 0.5 else 'black'
                ax.text(j, i, f'{val:.2f}', ha='center', va='center',
                        color=color, fontsize=7)

    fig.colorbar(im, ax=ax, label='Normalized Distance')
    fig.savefig(os.path.join(outdir, 'i9_input_diversity_heatmap.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i9_input_diversity_heatmap.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I9: i9_input_diversity_heatmap.pdf")


# ── I10: Oracle Stability ───────────────────────────────────────────────

def plot_i10_stability(metrics, outdir):
    """I10: Line plot of guidance Jaccard similarity vs round fraction."""
    m = find_metric(metrics, 'infra.i10.oracle_stability.incremental_convergence')
    if not m:
        print("  Skipping I10 stability: metric not found")
        return

    d = m['details']
    curve = d.get('convergence_curve', [])

    if not curve:
        print("  Skipping I10 stability: no convergence curve data")
        return

    rounds = [pt['round_count'] for pt in curve]
    jaccards = [pt['jaccard_with_full'] for pt in curve]
    avoid_counts = [pt['avoid_count'] for pt in curve]
    seek_counts = [pt['seek_count'] for pt in curve]

    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(12, 4.5))

    # Left: Jaccard similarity vs rounds
    ax1.plot(rounds, jaccards, 'o-', color='#2196F3', linewidth=2, markersize=8)
    ax1.axhline(y=0.8, color='gray', linestyle='--', alpha=0.5, label='80% threshold')
    ax1.set_xlabel('Rounds Included')
    ax1.set_ylabel('Jaccard Similarity with Full')
    ax1.set_title('I10: Guidance Convergence')
    ax1.set_ylim(-0.05, 1.05)
    ax1.legend()

    # Right: Avoid/seek counts vs rounds
    ax2.plot(rounds, avoid_counts, 's-', color='#f44336', linewidth=2, markersize=8, label='Avoid')
    ax2.plot(rounds, seek_counts, 'D-', color='#4CAF50', linewidth=2, markersize=8, label='Seek')
    ax2.set_xlabel('Rounds Included')
    ax2.set_ylabel('Token Count')
    ax2.set_title('I10: Guidance Token Counts over Rounds')
    ax2.legend()

    fig.tight_layout()
    fig.savefig(os.path.join(outdir, 'i10_oracle_stability.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i10_oracle_stability.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I10: i10_oracle_stability.pdf")


# ── I11: Selector Comparison ───────────────────────────────────────────

def plot_i11_selectors(metrics, outdir):
    """I11: Grouped bar of mutation pool coverage by selector + diversity."""
    m_cov = find_metric(metrics, 'infra.i11.selector_comparison.coverage_by_selector')
    m_div = find_metric(metrics, 'infra.i11.selector_comparison.diversity_by_selector')
    if not m_cov:
        print("  Skipping I11 selectors: metrics not found")
        return

    cov_data = m_cov['details']['by_selector']

    fig, axes = plt.subplots(1, 2, figsize=(12, 5))

    # Left: coverage bar
    selectors = [e['selector'] for e in cov_data]
    coverages = [e['coverage'] for e in cov_data]
    unique_sets = [e['unique_sets'] for e in cov_data]

    colors = ['#2196F3', '#FF9800', '#4CAF50', '#9E9E9E']
    bars = axes[0].bar(selectors, coverages, color=colors[:len(selectors)], alpha=0.85)
    axes[0].set_ylabel('Mutation Pool Coverage')
    axes[0].set_title('I11: Pool Coverage by Selector')
    axes[0].set_ylim(0, 1.1)

    for bar, val, us in zip(bars, coverages, unique_sets):
        axes[0].text(bar.get_x() + bar.get_width() / 2, bar.get_height() + 0.02,
                     f'{val:.2f}\n({us} sets)', ha='center', fontsize=8)

    # Right: diversity (if available)
    if m_div:
        div_data = m_div['details']['by_selector']
        div_selectors = [e['selector'] for e in div_data]
        diversities = [e['diversity'] for e in div_data]
        recipe_sizes = [e['mean_recipe_size'] for e in div_data]

        x = np.arange(len(div_selectors))
        axes[1].bar(x - 0.2, diversities, 0.35, label='Diversity (Jaccard)',
                    color='#2196F3', alpha=0.8)
        axes[1].bar(x + 0.2, [r / max(max(recipe_sizes, default=1), 1) for r in recipe_sizes],
                    0.35, label='Recipe Size (normalized)', color='#FF9800', alpha=0.8)
        axes[1].set_xticks(x)
        axes[1].set_xticklabels(div_selectors)
        axes[1].set_ylabel('Score')
        axes[1].set_title('I11: Selection Diversity & Recipe Size')
        axes[1].legend()

    fig.tight_layout()
    fig.savefig(os.path.join(outdir, 'i11_selector_comparison.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i11_selector_comparison.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I11: i11_selector_comparison.pdf")


# ── I11b: Mutation Frequency Heatmap ────────────────────────────────────

def plot_i11_heatmap(metrics, outdir):
    """I11: Heatmap of mutation selection frequency (mutation x selector)."""
    m = find_metric(metrics, 'infra.i11.selector_comparison.mutation_frequency_heatmap')
    if not m:
        print("  Skipping I11 heatmap: metric not found")
        return

    d = m['details']
    mutations = d.get('mutations', [])
    selectors = d.get('selectors', [])
    frequencies = d.get('frequencies', [])

    if not mutations or not selectors or not frequencies:
        print("  Skipping I11 heatmap: insufficient data")
        return

    # frequencies is selectors x mutations; transpose to mutations x selectors for display
    freq_matrix = np.array(frequencies).T  # shape: (n_mutations, n_selectors)

    fig, ax = plt.subplots(figsize=(10, max(6, len(mutations) * 0.35)))

    try:
        import seaborn as sns
        sns.heatmap(freq_matrix, annot=True, fmt='d', cmap='YlOrRd',
                    xticklabels=selectors, yticklabels=[m.replace('ast.', '') for m in mutations],
                    ax=ax, cbar_kws={'label': 'Selection Count'})
    except ImportError:
        im = ax.imshow(freq_matrix, cmap='YlOrRd', aspect='auto')
        ax.set_xticks(range(len(selectors)))
        ax.set_xticklabels(selectors)
        ax.set_yticks(range(len(mutations)))
        ax.set_yticklabels([m.replace('ast.', '') for m in mutations], fontsize=8)
        for i in range(len(mutations)):
            for j in range(len(selectors)):
                ax.text(j, i, str(int(freq_matrix[i, j])), ha='center', va='center', fontsize=8)
        fig.colorbar(im, ax=ax, label='Selection Count')

    ax.set_title('I11: Mutation Selection Frequency by Selector')
    ax.set_xlabel('Selector')
    ax.set_ylabel('Mutation')

    fig.tight_layout()
    fig.savefig(os.path.join(outdir, 'i11_mutation_heatmap.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i11_mutation_heatmap.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I11: i11_mutation_heatmap.pdf")


# ── I11c: Coverage & Diversity Trajectories ─────────────────────────────

def plot_i11_trajectories(metrics, outdir):
    """I11: 1x2 panel — cumulative coverage + rolling diversity over rounds per selector."""
    m_cov = find_metric(metrics, 'infra.i11.selector_comparison.coverage_trajectory')
    m_div = find_metric(metrics, 'infra.i11.selector_comparison.diversity_trajectory')
    m_sat = find_metric(metrics, 'infra.i11.selector_comparison.coverage_saturation')

    if not m_cov and not m_div:
        print("  Skipping I11 trajectories: metrics not found")
        return

    selector_style = {
        'Coverage': ('#2196F3', 'o'), 'Fuzzer': ('#FF9800', 's'),
        'Token': ('#4CAF50', 'D'), 'Random': ('#9E9E9E', '^'),
    }

    fig, axes = plt.subplots(1, 2, figsize=(14, 5))

    # Left: cumulative coverage
    if m_cov:
        by_sel = m_cov['details'].get('by_selector', [])
        for entry in by_sel:
            name = entry['selector']
            traj = entry['trajectory']
            rounds = [pt['round'] for pt in traj]
            coverage = [pt['coverage'] for pt in traj]
            color, marker = selector_style.get(name, ('#000000', 'x'))
            label = name
            # Add saturation round annotation
            if m_sat:
                sat_data = m_sat['details'].get('by_selector', [])
                for s in sat_data:
                    if s['selector'] == name and s.get('saturation_round') is not None:
                        label = f'{name} (sat@r{s["saturation_round"]})'
                        break
            axes[0].plot(rounds, coverage, marker=marker, color=color,
                        linewidth=1.5, markersize=4, label=label, alpha=0.85)
        axes[0].set_xlabel('Round')
        axes[0].set_ylabel('Cumulative Coverage')
        axes[0].set_title('Cumulative Mutation Pool Coverage')
        axes[0].set_ylim(-0.05, 1.05)
        axes[0].axhline(y=0.8, color='red', linestyle=':', alpha=0.5, linewidth=1, label='80% threshold')
        axes[0].legend(fontsize=8)

    # Right: rolling diversity
    if m_div:
        by_sel = m_div['details'].get('by_selector', [])
        for entry in by_sel:
            name = entry['selector']
            traj = entry['trajectory']
            rounds = [pt['round'] for pt in traj]
            diversity = [pt['diversity'] for pt in traj]
            color, marker = selector_style.get(name, ('#000000', 'x'))
            axes[1].plot(rounds, diversity, marker=marker, color=color,
                        linewidth=1.5, markersize=4, label=name, alpha=0.85)
        axes[1].set_xlabel('Round')
        axes[1].set_ylabel('Rolling Jaccard Diversity')
        axes[1].set_title('Rolling Diversity (window=5)')
        axes[1].set_ylim(-0.05, 1.05)
        axes[1].legend(fontsize=8)

    fig.suptitle('I11: Selector Trajectories', fontsize=13)
    fig.tight_layout()
    fig.savefig(os.path.join(outdir, 'i11_selector_trajectories.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i11_selector_trajectories.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I11: i11_selector_trajectories.pdf")


# ── I12: Guidance Utilization ───────────────────────────────────────────

def plot_i12_guidance(metrics, outdir):
    """I12: 2x2 grid of mutation frequency with/without guidance for all 4 selectors."""
    m = find_metric(metrics, 'infra.i12.guidance_utilization.recipe_delta')
    if not m:
        print("  Skipping I12 guidance: metric not found")
        return

    d = m['details']
    by_selector = d.get('by_selector', [])

    if not by_selector:
        print("  Skipping I12 guidance: no selector data")
        return

    n_selectors = len(by_selector)
    ncols = min(n_selectors, 2)
    nrows = (n_selectors + ncols - 1) // ncols
    fig, axes = plt.subplots(nrows, ncols, figsize=(14, 5 * nrows))
    if n_selectors == 1:
        axes = np.array([axes])
    axes = np.atleast_2d(axes)

    selector_colors = {
        'Coverage': '#2196F3', 'Fuzzer': '#FF9800',
        'Token': '#4CAF50', 'Random': '#9E9E9E',
    }

    for idx, sel in enumerate(by_selector):
        row, col = divmod(idx, ncols)
        ax = axes[row, col]
        freq_data = sel.get('mutation_frequencies', [])
        if not freq_data:
            ax.set_visible(False)
            continue

        mutations = [f['mutation'].replace('ast.', '')[:20] for f in freq_data]
        without = [f['without_guidance'] for f in freq_data]
        with_g = [f['with_guidance'] for f in freq_data]

        x = np.arange(len(mutations))
        ax.barh(x - 0.2, without, 0.35, label='Without Guidance', color='#9E9E9E', alpha=0.8)
        color = selector_colors.get(sel['selector'], '#2196F3')
        ax.barh(x + 0.2, with_g, 0.35, label='With Guidance', color=color, alpha=0.8)
        ax.set_yticks(x)
        ax.set_yticklabels(mutations, fontsize=7)
        ax.set_xlabel('Selection Frequency')
        ax.set_title(f'{sel["selector"]}')
        ax.legend(fontsize=7)

    # Hide any unused subplots
    for idx in range(n_selectors, nrows * ncols):
        row, col = divmod(idx, ncols)
        axes[row, col].set_visible(False)

    fig.suptitle('I12: Mutation Frequency With/Without Guidance by Selector', fontsize=13)
    fig.tight_layout()
    fig.savefig(os.path.join(outdir, 'i12_guidance_utilization.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i12_guidance_utilization.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I12: i12_guidance_utilization.pdf")


# ── I13: Convergence Simulation ─────────────────────────────────────────

def plot_i13_convergence(metrics, outdir):
    """I13: Multi-panel: recipe size + diversity + score vs round."""
    m_growth = find_metric(metrics, 'infra.i13.convergence_simulation.recipe_growth_rate')
    m_div = find_metric(metrics, 'infra.i13.convergence_simulation.diversity_preservation')
    m_score = find_metric(metrics, 'infra.i13.convergence_simulation.score_plateau_round')
    m_phase = find_metric(metrics, 'infra.i13.convergence_simulation.phase_transition_round')

    if not m_growth:
        print("  Skipping I13 convergence: metrics not found")
        return

    fig, axes = plt.subplots(1, 3, figsize=(16, 4.5))

    # Phase transition lines
    transitions = []
    if m_phase:
        transitions = m_phase['details'].get('all_transitions', [])

    phase_colors = {
        'Baseline': '#9E9E9E',
        'IndividualExploration': '#FF9800',
        'Accumulation': '#4CAF50',
    }

    # Panel 1: Recipe size trajectory
    recipe_data = m_growth['details'].get('recipe_trajectory', [])
    if recipe_data:
        rounds_r = [pt['round'] for pt in recipe_data]
        sizes = [pt['recipe_size'] for pt in recipe_data]
        axes[0].plot(rounds_r, sizes, 'o-', color='#2196F3', linewidth=2, markersize=4)
        axes[0].set_xlabel('Round')
        axes[0].set_ylabel('Recipe Size')
        axes[0].set_title('Recipe Growth')
        for t in transitions:
            axes[0].axvline(x=t['round'], color=phase_colors.get(t['phase'], 'gray'),
                           linestyle='--', alpha=0.7, linewidth=1)

    # Panel 2: Diversity trajectory
    if m_div:
        div_data = m_div['details'].get('diversity_trajectory', [])
        if div_data:
            rounds_d = [pt['round'] for pt in div_data]
            diversities = [pt['diversity'] for pt in div_data]
            axes[1].plot(rounds_d, diversities, 's-', color='#FF9800', linewidth=2, markersize=4)
            axes[1].set_xlabel('Round')
            axes[1].set_ylabel('Recipe Diversity')
            axes[1].set_title('Diversity over Rounds')
            axes[1].set_ylim(-0.05, 1.05)
            for t in transitions:
                axes[1].axvline(x=t['round'], color=phase_colors.get(t['phase'], 'gray'),
                               linestyle='--', alpha=0.7, linewidth=1)

    # Panel 3: Score trajectory
    if m_score:
        score_data = m_score['details'].get('score_trajectory', [])
        if score_data:
            rounds_s = [pt['round'] for pt in score_data]
            scores = [pt['score'] for pt in score_data]
            axes[2].plot(rounds_s, scores, 'D-', color='#4CAF50', linewidth=2, markersize=4)
            axes[2].set_xlabel('Round')
            axes[2].set_ylabel('Best Evasion Score')
            axes[2].set_title('Score Convergence')
            for t in transitions:
                axes[2].axvline(x=t['round'], color=phase_colors.get(t['phase'], 'gray'),
                               linestyle='--', alpha=0.7, linewidth=1)

            # Mark plateau
            plateau_round = m_score['details'].get('plateau_round', 0)
            if plateau_round > 0:
                axes[2].axvline(x=plateau_round, color='red', linestyle=':',
                               alpha=0.8, linewidth=2, label=f'Plateau (r={plateau_round})')
                axes[2].legend()

    # Add phase legend
    from matplotlib.lines import Line2D
    legend_elements = [Line2D([0], [0], color=c, linestyle='--', label=p)
                       for p, c in phase_colors.items()]
    axes[0].legend(handles=legend_elements, fontsize=7, loc='upper left')

    fig.tight_layout()
    fig.savefig(os.path.join(outdir, 'i13_convergence_simulation.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i13_convergence_simulation.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I13: i13_convergence_simulation.pdf")


# ── I13b: Per-Selector Convergence ──────────────────────────────────────

def plot_i13_convergence_by_selector(metrics, outdir):
    """I13: 1x3 panel — recipe size, diversity, best score per selector over rounds."""
    m = find_metric(metrics, 'infra.i13.convergence_simulation.selector_convergence')
    if not m:
        print("  Skipping I13 per-selector convergence: metric not found")
        return

    by_selector = m['details'].get('by_selector', [])
    if not by_selector:
        print("  Skipping I13 per-selector convergence: no selector data")
        return

    selector_style = {
        'Coverage': ('#2196F3', 'o'), 'Fuzzer': ('#FF9800', 's'),
        'Token': ('#4CAF50', 'D'), 'Random': ('#9E9E9E', '^'),
    }

    # Get phase transitions from the accumulation-only result
    m_phase = find_metric(metrics, 'infra.i13.convergence_simulation.phase_transition_round')
    transitions = []
    if m_phase:
        transitions = m_phase['details'].get('all_transitions', [])
    phase_colors = {
        'Baseline': '#9E9E9E',
        'IndividualExploration': '#FF9800',
        'Accumulation': '#4CAF50',
    }

    fig, axes = plt.subplots(1, 3, figsize=(16, 5))

    # Panel 1: Recipe size
    for entry in by_selector:
        name = entry['selector']
        traj = entry.get('recipe_size_trajectory', [])
        if not traj:
            continue
        rounds = [pt['round'] for pt in traj]
        sizes = [pt['recipe_size'] for pt in traj]
        color, marker = selector_style.get(name, ('#000000', 'x'))
        axes[0].plot(rounds, sizes, marker=marker, color=color,
                    linewidth=1.5, markersize=3, label=name, alpha=0.85)
    axes[0].set_xlabel('Round')
    axes[0].set_ylabel('Recipe Size')
    axes[0].set_title('Recipe Size by Selector')
    axes[0].legend(fontsize=8)
    for t in transitions:
        axes[0].axvline(x=t['round'], color=phase_colors.get(t['phase'], 'gray'),
                       linestyle='--', alpha=0.5, linewidth=1)

    # Panel 2: Diversity
    for entry in by_selector:
        name = entry['selector']
        traj = entry.get('diversity_trajectory', [])
        if not traj:
            continue
        rounds = [pt['round'] for pt in traj]
        diversity = [pt['diversity'] for pt in traj]
        color, marker = selector_style.get(name, ('#000000', 'x'))
        axes[1].plot(rounds, diversity, marker=marker, color=color,
                    linewidth=1.5, markersize=3, label=name, alpha=0.85)
    axes[1].set_xlabel('Round')
    axes[1].set_ylabel('Recipe Diversity')
    axes[1].set_title('Diversity by Selector')
    axes[1].set_ylim(-0.05, 1.05)
    axes[1].legend(fontsize=8)
    for t in transitions:
        axes[1].axvline(x=t['round'], color=phase_colors.get(t['phase'], 'gray'),
                       linestyle='--', alpha=0.5, linewidth=1)

    # Panel 3: Best score
    for entry in by_selector:
        name = entry['selector']
        traj = entry.get('score_trajectory', [])
        if not traj:
            continue
        rounds = [pt['round'] for pt in traj]
        scores = [pt['score'] for pt in traj]
        color, marker = selector_style.get(name, ('#000000', 'x'))
        axes[2].plot(rounds, scores, marker=marker, color=color,
                    linewidth=1.5, markersize=3, label=name, alpha=0.85)
    axes[2].set_xlabel('Round')
    axes[2].set_ylabel('Best Evasion Score')
    axes[2].set_title('Score Convergence by Selector')
    axes[2].legend(fontsize=8)
    for t in transitions:
        axes[2].axvline(x=t['round'], color=phase_colors.get(t['phase'], 'gray'),
                       linestyle='--', alpha=0.5, linewidth=1)

    # Add phase legend to first panel
    from matplotlib.lines import Line2D
    legend_elements = [Line2D([0], [0], color=c, linestyle='--', label=p)
                       for p, c in phase_colors.items()]
    ax_handles, ax_labels = axes[0].get_legend_handles_labels()
    axes[0].legend(handles=ax_handles + legend_elements, fontsize=7, loc='upper left')

    fig.suptitle('I13: Per-Selector Convergence Simulation', fontsize=13)
    fig.tight_layout()
    fig.savefig(os.path.join(outdir, 'i13_convergence_by_selector.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i13_convergence_by_selector.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I13: i13_convergence_by_selector.pdf")


# ── I14: Line Tracing Overhead ────────────────────────────────────────────

def plot_i14_line_tracing(metrics, outdir):
    """I14: Scatter of transform time vs source size + injection density bar."""
    m_scale = find_metric(metrics, 'infra.i14.line_tracing.scaling')
    m_density = find_metric(metrics, 'infra.i14.line_tracing.injection_density')
    if not m_scale and not m_density:
        print("  Skipping I14 line tracing: metrics not found")
        return

    fig, axes = plt.subplots(1, 2, figsize=(12, 5))

    # Left: transform time vs input lines (scaling)
    if m_scale:
        points = m_scale['details'].get('data_points', [])
        if points:
            x = [pt['input_lines'] for pt in points]
            y = [pt['mean_time_us'] for pt in points]
            axes[0].scatter(x, y, s=80, color='#2196F3', zorder=3, edgecolors='white')

            # Regression line
            slope = m_scale['details'].get('slope_us_per_line', 0)
            r2 = m_scale['details'].get('r_squared', 0)
            if len(x) >= 2:
                x_fit = np.linspace(min(x), max(x), 100)
                mean_y = np.mean(y)
                mean_x = np.mean(x)
                intercept = mean_y - slope * mean_x
                y_fit = slope * x_fit + intercept
                axes[0].plot(x_fit, y_fit, '--', color='#f44336', linewidth=2,
                            label=f'slope={slope:.2f} us/line, R²={r2:.3f}')
                axes[0].legend(fontsize=8)

            axes[0].set_xlabel('Input Lines')
            axes[0].set_ylabel('Transform Time (us)')
            axes[0].set_title('I14: Line Tracing Scaling')

    # Right: trace calls injected vs deferred per source
    if m_density:
        per_source = m_density['details'].get('per_source', [])
        if per_source:
            labels = [s['source'][:15] for s in per_source]
            eager = [s['trace_calls'] for s in per_source]
            deferred = [s['deferred_calls'] for s in per_source]

            x = np.arange(len(labels))
            axes[1].bar(x - 0.2, eager, 0.35, label='Eager Traces', color='#2196F3', alpha=0.85)
            axes[1].bar(x + 0.2, deferred, 0.35, label='Deferred (Loop)', color='#FF9800', alpha=0.85)
            axes[1].set_xticks(x)
            axes[1].set_xticklabels(labels, rotation=45, ha='right', fontsize=8)
            axes[1].set_ylabel('Count')
            axes[1].set_title('I14: Trace Injection Density')
            axes[1].legend()

    fig.tight_layout()
    fig.savefig(os.path.join(outdir, 'i14_line_tracing.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i14_line_tracing.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I14: i14_line_tracing.pdf")


# ── I15: SC Checkpoint Patching ──────────────────────────────────────────

def plot_i15_sc_checkpoints(metrics, outdir):
    """I15: 2x2 panel — size scaling, checkpoint scaling, clamping, throughput."""
    m_size = find_metric(metrics, 'infra.i15.sc_checkpoint.scaling_by_size')
    m_count = find_metric(metrics, 'infra.i15.sc_checkpoint.scaling_by_checkpoints')
    m_clamp = find_metric(metrics, 'infra.i15.sc_checkpoint.clamping_rate')
    m_tput = find_metric(metrics, 'infra.i15.sc_checkpoint.throughput_by_size')

    if not m_size and not m_tput:
        print("  Skipping I15 sc checkpoints: metrics not found")
        return

    fig, axes = plt.subplots(2, 2, figsize=(14, 10))

    # Top-left: log-log scatter of shellcode_size vs patch time (checkpoint_count=5)
    if m_size:
        points = m_size['details'].get('data_points', [])
        if points:
            sizes = [pt['shellcode_size'] for pt in points]
            times = [pt['mean_patch_us'] for pt in points]
            axes[0, 0].scatter(sizes, times, s=80, color='#2196F3', zorder=3, edgecolors='white')
            axes[0, 0].set_xscale('log')
            axes[0, 0].set_yscale('log')
            axes[0, 0].set_xlabel('Shellcode Size (bytes)')
            axes[0, 0].set_ylabel('Patch Time (us)')
            axes[0, 0].set_title('I15: Patch Time vs Size (5 checkpoints)')

            # Regression line in log-log
            slope = m_size['details'].get('slope_us_per_byte', 0)
            r2 = m_size['details'].get('r_squared', 0)
            if len(sizes) >= 2:
                x_fit = np.logspace(np.log10(min(sizes)), np.log10(max(sizes)), 50)
                mean_y = np.mean(times)
                mean_x = np.mean(sizes)
                intercept = mean_y - slope * mean_x
                y_fit = slope * x_fit + intercept
                y_fit = np.maximum(y_fit, 0.1)  # clip for log scale
                axes[0, 0].plot(x_fit, y_fit, '--', color='#f44336', linewidth=2,
                               label=f'slope={slope:.4f}, R²={r2:.3f}')
                axes[0, 0].legend(fontsize=8)

    # Top-right: line plot of checkpoint count vs patch time for mid-size shellcode
    if m_count:
        points = m_count['details'].get('data_points', [])
        ref_file = m_count['details'].get('reference_file', '')
        if points:
            counts = [pt['checkpoint_count'] for pt in points]
            times = [pt['mean_patch_us'] for pt in points]
            axes[0, 1].plot(counts, times, 'o-', color='#4CAF50', linewidth=2, markersize=8)
            axes[0, 1].set_xlabel('Requested Checkpoints')
            axes[0, 1].set_ylabel('Patch Time (us)')
            axes[0, 1].set_title(f'I15: Patch Time vs Count ({ref_file[:20]})')

            slope = m_count['details'].get('slope_us_per_checkpoint', 0)
            axes[0, 1].text(0.05, 0.95, f'slope={slope:.2f} us/checkpoint',
                           transform=axes[0, 1].transAxes, fontsize=9,
                           verticalalignment='top',
                           bbox=dict(boxstyle='round', facecolor='wheat', alpha=0.5))

    # Bottom-left: clamping heatmap
    if m_clamp and m_tput:
        per_file = m_tput['details'].get('per_file', [])
        if per_file:
            # Build matrix: shellcode x checkpoint_count -> actual
            shellcodes = sorted(set(r['shellcode'] for r in per_file),
                              key=lambda s: next((r['size'] for r in per_file if r['shellcode'] == s), 0))
            ckpts = sorted(set(int(r['checkpoints']) for r in per_file))

            matrix = np.zeros((len(shellcodes), len(ckpts)))
            for r in per_file:
                si = shellcodes.index(r['shellcode'])
                ci = ckpts.index(int(r['checkpoints']))
                # Show clamping: requested - actual (0 = no clamping)
                requested = int(r['checkpoints'])
                # We need actual from the raw data; approximate from throughput
                # Use the full per_file data
                matrix[si, ci] = r.get('bytes_per_us', 0)

            im = axes[1, 0].imshow(matrix, cmap='YlGn', aspect='auto')
            axes[1, 0].set_xticks(range(len(ckpts)))
            axes[1, 0].set_xticklabels(ckpts)
            axes[1, 0].set_yticks(range(len(shellcodes)))
            short_names = [s.replace('.bin', '')[:15] for s in shellcodes]
            axes[1, 0].set_yticklabels(short_names, fontsize=8)
            axes[1, 0].set_xlabel('Checkpoint Count')
            axes[1, 0].set_ylabel('Shellcode')
            axes[1, 0].set_title('I15: Throughput (bytes/us)')
            fig.colorbar(im, ax=axes[1, 0])

    # Bottom-right: throughput bar by size category
    if m_tput:
        per_file = m_tput['details'].get('per_file', [])
        if per_file:
            # Group by shellcode, average throughput
            sc_throughput = {}
            sc_sizes = {}
            for r in per_file:
                name = r['shellcode']
                if name not in sc_throughput:
                    sc_throughput[name] = []
                    sc_sizes[name] = r['size']
                sc_throughput[name].append(r['bytes_per_us'])

            names = sorted(sc_throughput.keys(), key=lambda n: sc_sizes[n])
            means = [np.mean(sc_throughput[n]) for n in names]
            short_names = [n.replace('.bin', '')[:12] for n in names]
            sizes_label = [f'{sc_sizes[n]}B' if sc_sizes[n] < 1024
                          else f'{sc_sizes[n]//1024}KB' for n in names]

            bars = axes[1, 1].bar(range(len(names)), means, color='#FF9800', alpha=0.85)
            axes[1, 1].set_xticks(range(len(names)))
            axes[1, 1].set_xticklabels([f'{n}\n{s}' for n, s in zip(short_names, sizes_label)],
                                        fontsize=7, rotation=45, ha='right')
            axes[1, 1].set_ylabel('Throughput (bytes/us)')
            axes[1, 1].set_title('I15: Disassembly Throughput by Shellcode')

    fig.tight_layout()
    fig.savefig(os.path.join(outdir, 'i15_sc_checkpoint.pdf'), bbox_inches='tight')
    fig.savefig(os.path.join(outdir, 'i15_sc_checkpoint.png'), bbox_inches='tight')
    plt.close(fig)
    print("  I15: i15_sc_checkpoint.pdf")


# ── Infrastructure Summary Table ─────────────────────────────────────────

def generate_infra_summary_table(metrics, outdir):
    """Generate a LaTeX summary table of all infrastructure metrics."""
    rows = []
    for mid, m in sorted(metrics.items()):
        if not mid.startswith('infra.'):
            continue
        rows.append({
            'id': mid.replace('infra.', ''),
            'label': m['label'][:60],
            'value': m['value'],
            'n': m['n'],
        })

    if not rows:
        print("  Skipping infra summary: no infra metrics found")
        return

    latex_path = os.path.join(outdir, 'infra_metrics_table.tex')
    with open(latex_path, 'w') as f:
        f.write('\\begin{table}[htbp]\n')
        f.write('\\centering\n')
        f.write('\\caption{Infrastructure-Level Evaluation Metrics}\n')
        f.write('\\label{tab:infra-metrics}\n')
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
            mid_tex = row['id'].replace('_', '\\_')
            f.write(f"\\texttt{{{mid_tex}}} & {label} & {row['value']:.4f} & {row['n']} \\\\\n")

        f.write('\\bottomrule\n')
        f.write('\\end{tabular}\n')
        f.write('\\end{table}\n')

    print(f"  Infra Summary: {latex_path}")


# ── Main ──────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description='Generate evaluation figures')
    parser.add_argument('--input', default=None,
                        help='Input JSON report (default: auto-detect based on mode)')
    parser.add_argument('--outdir', default='figures',
                        help='Output directory for figures')
    parser.add_argument('--infra', action='store_true',
                        help='Generate infrastructure-level plots (I1-I8)')
    args = parser.parse_args()

    # Auto-detect input based on mode
    if args.input is None:
        args.input = 'infra_eval_report.json' if args.infra else 'component_eval_report.json'

    if not os.path.exists(args.input):
        print(f"Error: {args.input} not found")
        if args.infra:
            print("Run: cargo run -p evaluation --bin infra-eval")
        else:
            print("Run: cargo run -p evaluation --features full --bin component-eval")
        sys.exit(1)

    os.makedirs(args.outdir, exist_ok=True)
    metrics = load_report(args.input)
    print(f"Loaded {len(metrics)} metrics from {args.input}")
    print(f"Output directory: {args.outdir}\n")

    if args.infra:
        # Infrastructure plots
        print("Generating infrastructure plots:")
        plot_i1_entropy(metrics, args.outdir)
        plot_i1_expansion(metrics, args.outdir)
        plot_i2_impact(metrics, args.outdir)
        plot_i3_ir_analysis(metrics, args.outdir)
        plot_i4_pe_heatmap(metrics, args.outdir)
        plot_i5_assembly(metrics, args.outdir)
        plot_i6_overhead(metrics, args.outdir)
        plot_i7_tokens(metrics, args.outdir)
        plot_i8_scoring(metrics, args.outdir)
        plot_i9_diversity(metrics, args.outdir)
        plot_i10_stability(metrics, args.outdir)
        plot_i11_selectors(metrics, args.outdir)
        plot_i11_heatmap(metrics, args.outdir)
        plot_i11_trajectories(metrics, args.outdir)
        plot_i12_guidance(metrics, args.outdir)
        plot_i13_convergence(metrics, args.outdir)
        plot_i13_convergence_by_selector(metrics, args.outdir)
        plot_i14_line_tracing(metrics, args.outdir)
        plot_i15_sc_checkpoints(metrics, args.outdir)

        print("\nGenerating tables:")
        generate_infra_summary_table(metrics, args.outdir)
    else:
        # Component plots
        print("Generating component plots:")
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

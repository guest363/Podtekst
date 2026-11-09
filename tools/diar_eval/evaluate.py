#!/usr/bin/env python3
"""Compare reference and hypothesis RTTM with pyannote.metrics.

Dev-only: the runtime never needs Python. Install the metric once:
    pip install pyannote.metrics
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("hypothesis", type=Path)
    parser.add_argument("--collar", type=float, default=0.0)
    parser.add_argument("--skip-overlap", action="store_true")
    parser.add_argument("--per-file", action="store_true")
    args = parser.parse_args()
    try:
        from pyannote.database.util import load_rttm
        from pyannote.metrics.diarization import DiarizationErrorRate
    except ImportError:
        sys.exit("нужен pyannote.metrics: pip install pyannote.metrics")

    reference = load_rttm(str(args.reference))
    hypothesis = load_rttm(str(args.hypothesis))
    common = sorted(set(reference) & set(hypothesis))
    if not common:
        sys.exit(
            "нет общих file id в RTTM. При конвертации укажи "
            "--id как в эталонной разметке."
        )
    metric = DiarizationErrorRate(
        collar=args.collar,
        skip_overlap=args.skip_overlap,
    )
    for uri in common:
        scores = metric(reference[uri], hypothesis[uri], detailed=args.per_file)
        if args.per_file:
            print(
                f"{uri}: DER={scores['diarization error rate']:.4f} "
                f"miss={scores['miss']:.4f} "
                f"fa={scores['false alarm']:.4f} "
                f"conf={scores['confusion']:.4f}"
            )
    print(
        f"files={len(common)} DER={abs(metric):.4f} "
        f"collar={args.collar:.2f} skip_overlap={args.skip_overlap}"
    )


if __name__ == "__main__":
    main()

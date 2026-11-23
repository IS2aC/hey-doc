"""Usage: uv run python -m heydoc_pipeline <étape> [options]"""

import argparse
import importlib

STEPS = {
    "site": "site_gde",
    "airp": "airp",
    "mugef": "mugef",
    "cmu-hta": "cmu_hta",
    "dedup": "dedup",
    "load": "load",
    "gmaps-match": "gmaps",
}


def main() -> None:
    p = argparse.ArgumentParser(prog="heydoc_pipeline", description="Pipeline de données Hey-Doc")
    p.add_argument("step", choices=[*STEPS, "all"])
    p.add_argument("--only", help="sous-partie d'une étape (site : duties, tables, directory, details)")
    p.add_argument("--limit", type=int, help="nombre maximal d'éléments, pour un essai")
    args = p.parse_args()
    # gmaps-match needs a manual search and review first: `all` leaves it out.
    for step in [s for s in STEPS if s != "gmaps-match"] if args.step == "all" else [args.step]:
        print(f"== {step}", flush=True)
        mod = importlib.import_module(f"heydoc_pipeline.{STEPS[step]}")
        kwargs = {k: v for k, v in (("only", args.only), ("limit", args.limit)) if v is not None}
        mod.run(**kwargs) if step in ("site",) else mod.run()


if __name__ == "__main__":
    main()

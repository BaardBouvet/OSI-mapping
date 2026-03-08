#!/usr/bin/env python3
"""
Validate OSI mapping files: JSON Schema + semantic consistency checks.

Usage:
    python validation/validate.py                     # all examples in ./examples/
    python validation/validate.py path/to/mapping.yaml  # single file
    python validation/validate.py --all path/to/dir/    # all mapping.yaml in dir tree
"""
import json, yaml, sys, argparse
from pathlib import Path
from collections import Counter
from jsonschema import Draft202012Validator

try:
    import sqlglot
    HAS_SQLGLOT = True
except ImportError:
    HAS_SQLGLOT = False


# ──────────────────────────────────────────────────────────────────────
# Helpers
# ──────────────────────────────────────────────────────────────────────

def _resolve_target_field(tf):
    """Normalize a TargetField (string shorthand or dict) → (strategy, dict)."""
    if isinstance(tf, str):
        return tf, {}
    return tf.get("strategy"), tf


def _mapping_target_name(m):
    """Return the target entity name for a mapping (string or DatasetRef)."""
    t = m.get("target")
    if isinstance(t, str):
        return t
    if isinstance(t, dict):
        return t.get("dataset")
    return None


def _source_datasets(doc):
    """Return set of all source dataset names mentioned in mappings."""
    ds = set()
    for m in doc.get("mappings") or []:
        src = m.get("source") or {}
        if isinstance(src, dict) and "dataset" in src:
            ds.add(src["dataset"])
    return ds


def _expr_str(expr):
    """Extract a plain string from an Expression."""
    if isinstance(expr, str):
        return expr
    return None


# ──────────────────────────────────────────────────────────────────────
# Validation passes
# ──────────────────────────────────────────────────────────────────────

def pass_schema(doc, validator):
    """Pass 1 — JSON Schema validation."""
    errors = []
    for e in validator.iter_errors(doc):
        path = " > ".join(str(p) for p in e.absolute_path) or "(root)"
        errors.append(f"[Schema] [{path}] {e.message}")
    return errors


def pass_unique_names(doc):
    """Pass 2 — Unique mapping names, unique field targets per mapping."""
    errors = []
    mappings = doc.get("mappings") or []

    # 2a: mapping names must be unique
    names = [m["name"] for m in mappings if "name" in m]
    for name, count in Counter(names).items():
        if count > 1:
            errors.append(f"[Unique] Mapping name '{name}' appears {count} times")

    # 2b: within each mapping, (source, target) field pairs should be unique
    for m in mappings:
        field_targets = []
        for fm in m.get("fields") or []:
            src = fm.get("source", "<none>")
            tgt = fm.get("target", "<none>")
            field_targets.append((src, tgt))
        for pair, count in Counter(field_targets).items():
            if count > 1 and pair[1] != "<none>":
                errors.append(
                    f"[Unique] Mapping '{m.get('name')}': field target "
                    f"'{pair[1]}' (source '{pair[0]}') appears {count} times"
                )

    return errors


def pass_target_refs(doc):
    """Pass 3 — Target entity and field reference integrity."""
    errors = []
    targets = doc.get("targets") or {}
    target_names = set(targets.keys())
    mappings = doc.get("mappings") or []

    # 3a: mapping.target must reference a defined target (when string)
    for m in mappings:
        tname = _mapping_target_name(m)
        if isinstance(m.get("target"), str) and tname not in target_names:
            errors.append(
                f"[Reference] Mapping '{m.get('name')}': target '{tname}' "
                f"not found in targets ({', '.join(sorted(target_names)) or 'none'})"
            )

    # 3b: target field references must point to other targets
    for tname, tdef in targets.items():
        for fname, fdef in (tdef.get("fields") or {}).items():
            _, fd = _resolve_target_field(fdef)
            ref = fd.get("references") if isinstance(fd, dict) else None
            if ref and ref not in target_names:
                errors.append(
                    f"[Reference] Target '{tname}.{fname}': references "
                    f"'{ref}' not found in targets"
                )

    return errors


def pass_strategy_consistency(doc):
    """Pass 4 — Strategy / field mapping consistency.

    - expression strategy requires expression on target field
    - link_group fields should use identity strategy
    - group fields should use last_modified strategy
    - coalesce fields: warn if contributing mappings lack priority
    - last_modified fields: warn if contributing mappings lack timestamp
    """
    errors = []
    warnings = []
    targets = doc.get("targets") or {}
    mappings = doc.get("mappings") or []

    # Index: target_field → list of (mapping_name, field_mapping)
    contributions = {}  # (target_name, field_name) → [(mapping_name, fm)]
    for m in mappings:
        tname = _mapping_target_name(m)
        for fm in m.get("fields") or []:
            ftarget = fm.get("target")
            if ftarget and tname:
                key = (tname, ftarget)
                contributions.setdefault(key, []).append((m.get("name"), m, fm))

    for tname, tdef in targets.items():
        for fname, fdef in (tdef.get("fields") or {}).items():
            strategy, fd = _resolve_target_field(fdef)

            # 4a: expression strategy must have expression
            if strategy == "expression" and isinstance(fd, dict):
                if not fd.get("expression"):
                    errors.append(
                        f"[Strategy] Target '{tname}.{fname}': strategy "
                        f"'expression' requires an 'expression'"
                    )

            # 4b: link_group → should be identity
            if isinstance(fd, dict) and fd.get("link_group"):
                if strategy != "identity":
                    errors.append(
                        f"[Strategy] Target '{tname}.{fname}': link_group "
                        f"requires strategy 'identity', got '{strategy}'"
                    )

            # 4c: group → should be last_modified
            if isinstance(fd, dict) and fd.get("group"):
                if strategy not in ("last_modified", "coalesce"):
                    warnings.append(
                        f"[Strategy] Target '{tname}.{fname}': group is "
                        f"typically used with 'last_modified' strategy, "
                        f"got '{strategy}'"
                    )

            # 4d: coalesce — contributing mappings should have priority
            key = (tname, fname)
            contribs = contributions.get(key, [])
            if strategy == "coalesce" and len(contribs) > 1:
                for mname, mdef, fm in contribs:
                    has_priority = (
                        fm.get("priority") is not None
                        or mdef.get("priority") is not None
                    )
                    if not has_priority:
                        warnings.append(
                            f"[Strategy] Mapping '{mname}' → "
                            f"'{tname}.{fname}': coalesce strategy but "
                            f"no priority set"
                        )

            # 4e: last_modified — contributing mappings should have timestamp
            if strategy == "last_modified" and len(contribs) > 1:
                for mname, mdef, fm in contribs:
                    has_ts = (
                        fm.get("last_modified") is not None
                        or mdef.get("last_modified") is not None

                    )
                    if not has_ts:
                        warnings.append(
                            f"[Strategy] Mapping '{mname}' → "
                            f"'{tname}.{fname}': last_modified strategy "
                            f"but no timestamp source"
                        )

    return errors, warnings


def pass_field_coverage(doc):
    """Pass 5 — Field mapping ↔ target field coverage.

    - Every mapping field that targets a field must map to an existing target field
    - Warn if a target field has no contributing mapping
    """
    errors = []
    warnings = []
    targets = doc.get("targets") or {}
    mappings = doc.get("mappings") or []

    # Fields actually contributed to by mappings
    contributed = set()  # (target_name, field_name)
    for m in mappings:
        tname = _mapping_target_name(m)
        if tname not in targets:
            continue  # already caught in pass 3
        target_fields = set((targets.get(tname) or {}).get("fields", {}).keys())
        for fm in m.get("fields") or []:
            ftarget = fm.get("target")
            if ftarget:
                contributed.add((tname, ftarget))
                if ftarget not in target_fields:
                    errors.append(
                        f"[Field] Mapping '{m.get('name')}': field target "
                        f"'{ftarget}' not found in target '{tname}' fields "
                        f"({', '.join(sorted(target_fields))})"
                    )

    # Warn about orphan target fields (defined but never mapped)
    for tname, tdef in targets.items():
        for fname in (tdef.get("fields") or {}):
            if (tname, fname) not in contributed:
                warnings.append(
                    f"[Field] Target '{tname}.{fname}': no mapping "
                    f"contributes to this field"
                )

    return errors, warnings


def pass_test_datasets(doc):
    """Pass 6 — Test case dataset names match mapping source datasets."""
    errors = []
    warnings = []
    source_ds = _source_datasets(doc)
    tests = doc.get("tests") or []

    for i, tc in enumerate(tests):
        desc = tc.get("description", f"test[{i}]")

        # Input datasets should match known source datasets
        for ds in tc.get("input", {}):
            if ds not in source_ds:
                warnings.append(
                    f"[Test] '{desc}': input dataset '{ds}' not found in "
                    f"mapping sources ({', '.join(sorted(source_ds))})"
                )

        # Expected datasets should match known source datasets
        for ds in tc.get("expected", {}):
            if ds not in source_ds:
                warnings.append(
                    f"[Test] '{desc}': expected dataset '{ds}' not found in "
                    f"mapping sources ({', '.join(sorted(source_ds))})"
                )

    return errors, warnings


def pass_sql_syntax(doc):
    """Pass 7 — SQL expression syntax (requires sqlglot)."""
    if not HAS_SQLGLOT:
        return [], ["[SQL] Warning: sqlglot not installed, skipping SQL validation"]

    errors = []
    targets = doc.get("targets") or {}
    mappings = doc.get("mappings") or []

    def check_expr(expr, location):
        s = _expr_str(expr)
        if not s:
            return
        try:
            sqlglot.transpile(f"SELECT {s}", error_level=sqlglot.ErrorLevel.RAISE)
        except sqlglot.errors.ParseError as e:
            errors.append(f"[SQL] {location}: {e}")

    # Target-level expressions
    for tname, tdef in targets.items():
        for fname, fdef in (tdef.get("fields") or {}).items():
            _, fd = _resolve_target_field(fdef)
            if isinstance(fd, dict):
                if fd.get("expression"):
                    check_expr(fd["expression"], f"target '{tname}.{fname}' expression")
                if fd.get("default_expression"):
                    check_expr(fd["default_expression"], f"target '{tname}.{fname}' default_expression")

    # Mapping-level expressions
    for m in mappings:
        mname = m.get("name", "?")
        if m.get("filter"):
            check_expr(m["filter"], f"mapping '{mname}' filter")
        if m.get("reverse_filter"):
            check_expr(m["reverse_filter"], f"mapping '{mname}' reverse_filter")

        for fm in m.get("fields") or []:
            flabel = fm.get("target") or fm.get("source") or "?"
            if fm.get("expression"):
                check_expr(fm["expression"], f"mapping '{mname}' field '{flabel}' expression")
            if fm.get("reverse_expression"):
                check_expr(fm["reverse_expression"], f"mapping '{mname}' field '{flabel}' reverse_expression")

    return errors, []


# ──────────────────────────────────────────────────────────────────────
# Main
# ──────────────────────────────────────────────────────────────────────

def validate_file(filepath, schema_validator, *, verbose=False):
    """Run all validation passes on one mapping file. Returns (errors, warnings)."""
    with open(filepath) as f:
        try:
            doc = yaml.safe_load(f)
        except yaml.YAMLError as e:
            return [f"[YAML] Parse error: {e}"], []

    all_errors = []
    all_warnings = []

    # Pass 1: JSON Schema
    all_errors.extend(pass_schema(doc, schema_validator))

    # Skip semantic passes if schema validation fails badly
    if len(all_errors) > 5:
        return all_errors, all_warnings

    # Pass 2: Unique names
    all_errors.extend(pass_unique_names(doc))

    # Pass 3: Target references
    all_errors.extend(pass_target_refs(doc))

    # Pass 4: Strategy consistency
    errs, warns = pass_strategy_consistency(doc)
    all_errors.extend(errs)
    all_warnings.extend(warns)

    # Pass 5: Field coverage
    errs, warns = pass_field_coverage(doc)
    all_errors.extend(errs)
    all_warnings.extend(warns)

    # Pass 6: Test dataset consistency
    errs, warns = pass_test_datasets(doc)
    all_errors.extend(errs)
    all_warnings.extend(warns)

    # Pass 7: SQL syntax
    errs, warns = pass_sql_syntax(doc)
    all_errors.extend(errs)
    all_warnings.extend(warns)

    return all_errors, all_warnings


def main():
    parser = argparse.ArgumentParser(description="Validate OSI mapping files")
    parser.add_argument("path", nargs="?", help="File or directory to validate")
    parser.add_argument("--all", action="store_true", help="Recursively find mapping.yaml in directory")
    parser.add_argument("-v", "--verbose", action="store_true", help="Show warnings")
    parser.add_argument("-q", "--quiet", action="store_true", help="Only show failures")
    args = parser.parse_args()

    repo_root = Path(__file__).resolve().parent.parent
    schema_path = repo_root / "spec" / "mapping-schema.json"

    with open(schema_path) as f:
        schema = json.load(f)
    schema_validator = Draft202012Validator(schema)

    # Collect files to validate
    if args.path:
        p = Path(args.path)
        if p.is_file():
            files = [p]
        elif p.is_dir():
            files = sorted(p.rglob("mapping.yaml"))
        else:
            print(f"Error: {p} not found")
            sys.exit(2)
    else:
        files = sorted((repo_root / "examples").rglob("mapping.yaml"))

    total_errors = 0
    total_warnings = 0
    checked = 0

    for filepath in files:
        checked += 1
        label = filepath.parent.name if filepath.name == "mapping.yaml" else filepath.name
        errors, warnings = validate_file(filepath, schema_validator, verbose=args.verbose)

        total_errors += len(errors)
        total_warnings += len(warnings)

        if errors:
            print(f"  FAIL {label}: {len(errors)} error(s), {len(warnings)} warning(s)")
            for e in errors:
                print(f"       {e}")
            if args.verbose:
                for w in warnings:
                    print(f"       {w}")
        elif warnings and args.verbose:
            print(f"  WARN {label}: {len(warnings)} warning(s)")
            for w in warnings:
                print(f"       {w}")
        elif not args.quiet:
            status = "OK  " if not warnings else f"OK   ({len(warnings)} warning{'s' if len(warnings) != 1 else ''})"
            print(f"  {status} {label}")

    print(f"\n{checked} checked, {total_errors} error(s), {total_warnings} warning(s)")
    if not HAS_SQLGLOT:
        print("  (sqlglot not installed — SQL syntax checks skipped)")
    sys.exit(1 if total_errors else 0)


if __name__ == "__main__":
    main()

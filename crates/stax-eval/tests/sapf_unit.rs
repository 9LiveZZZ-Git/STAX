use stax_eval::interp::Interp;
use stax_parser::parse;

fn try_run(src: &str) -> Option<bool> {
    let result = std::panic::catch_unwind(|| {
        let ops = parse(src).ok()?;
        let mut interp = Interp::new();
        interp.exec(&ops).ok()?;
        let top = interp.stack.pop()?;
        Some(top.is_truthy())
    });
    result.unwrap_or(None)
}

#[test]
fn sapf_unit_tests() {
    let _path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../outputs/scratch/sapf/unit-tests.txt"
    );

    // Normalise: the file is relative to the workspace root, which is two levels up
    // from the crate dir.  Use an absolute sibling path instead.
    let abs = {
        let crate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        // crate dir: …/stax/crates/stax-eval
        // workspace:  …/stax
        // sibling:    …/sapf/unit-tests.txt
        let workspace = crate_dir.parent().unwrap().parent().unwrap();
        workspace
            .parent()
            .unwrap()
            .join("sapf")
            .join("unit-tests.txt")
    };

    let raw =
        std::fs::read_to_string(&abs).unwrap_or_else(|e| panic!("Cannot read {:?}: {e}", abs));

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut failures = Vec::new();

    let mut test_num = 0usize;
    for line in raw.lines() {
        let t = line.trim();
        // Only handle lines that are a quoted test string
        if !t.starts_with('"') || !t.ends_with('"') || t.len() < 2 {
            continue;
        }
        let src = &t[1..t.len() - 1];
        test_num += 1;
        // Flush before running each test so we can see which test caused a crash
        use std::io::Write;
        print!("#{test_num}.. ");
        let _ = std::io::stdout().flush();
        match try_run(src) {
            Some(true) => passed += 1,
            Some(false) => {
                failed += 1;
                failures.push(format!("FAIL: {src}"));
            }
            None => {
                skipped += 1;
                failures.push(format!("ERR:  {src}"));
            }
        }
    }

    let total = passed + failed + skipped;
    let pct = if total > 0 { passed * 100 / total } else { 0 };
    println!("\nSAPF unit tests: {passed}/{total} ({pct}%)  failed={failed}  errors={skipped}");
    for f in &failures {
        println!("  {f}");
    }

    // M1 target: ≥30% pass rate
    assert!(pct >= 30, "Pass rate {pct}% is below M1 target of 30%");
}


    #[test]
    fn verbose_streams_live_activity_into_the_launcher() {
        // A lane's log is its only diagnostic. Before this flag existed the
        // launcher had no way to ask for live output, so a lane killed at its
        // timeout left a zero-byte log (three lanes, 2026-09-03).
        let args = parse(&[
            "edda",
            "--agent",
            "claude",
            "--prompt-file",
            "p.txt",
            "--verbose",
        ]);
        assert!(args.verbose, "--verbose must parse");
        let quiet = parse(&["edda", "--agent", "claude", "--prompt-file", "p.txt"]);
        assert!(!quiet.verbose, "quiet stays the default");
    }

    #[test]
    fn run_inner_refuses_verbose_with_json() {
        // --json promises exactly one JSON object on stdout; interleaved
        // activity lines would break every consumer of it.
        let args = parse(&[
            "edda",
            "--agent",
            "claude",
            "--prompt-file",
            "p.txt",
            "--verbose",
            "--json",
        ]);
        let error = run_inner(args).expect_err("--verbose with --json must be refused");
        assert!(error.to_string().contains("--verbose"), "{error}");
    }

#!/bin/sh
# Canary c1 fixture scenario note.
# The canary under review is diff.patch (a new-file diff); this fixture
# directory only records the scenario context so the canary is runnable
# without repo-specific knowledge.
#
# Scenario: a deploy helper that runs a fast build first and, on failure,
# is intended to clean up a temp dir and fall back to a slow build.

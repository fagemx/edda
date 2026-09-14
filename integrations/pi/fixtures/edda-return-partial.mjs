// A malformed-but-parseable `edda return claim` response: the mailbox must not
// present a record that is missing its required fields.
const owner = process.argv[process.argv.indexOf('--owner') + 1];
process.stdout.write(JSON.stringify({ status: 'claimed', owner, session: 'x', returns: [{ status: 'done' }], count: 1 }) + '\n');
process.exit(0);

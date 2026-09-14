// A deliberately malformed `edda` stand-in: the owner mailbox must fail closed
// on unparseable output instead of presenting partial content.
process.stdout.write('this is not json\n');
process.exit(0);

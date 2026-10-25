// A stand-in for noura-kit-host that says it's ready, then stops with
// code 3 on the first request it reads, as a host that crashes does.
import { createInterface } from 'node:readline';

console.log(JSON.stringify({ ready: true }));
createInterface({ input: process.stdin }).once('line', () => {
	process.stderr.write('The stand-in host stopped on purpose.\n', () =>
		process.exit(3),
	);
});

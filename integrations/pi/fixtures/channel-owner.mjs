import { startChannel } from '../channel.mjs';
const [root, sessionId] = process.argv.slice(2);
const channel = await startChannel({ root, sessionId, cwd: root, deliver() {} });
console.log(JSON.stringify({ instanceId: channel.instanceId }));
// Deliberate test owner lifetime: parent kills this process to exercise crash recovery.
setInterval(() => {}, 1000);

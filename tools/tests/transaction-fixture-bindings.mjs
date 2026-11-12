// Extracted App/QueryTabView fixtures lose their imports. Bind the actual module,
// including its catalog/driver capability checks, rather than replacing its gates.
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';

const bundle = await build({
  entryPoints: [fileURLToPath(new URL('../../src/core/manualTransaction.ts', import.meta.url))],
  bundle: true, write: false, platform: 'node', format: 'esm', logLevel: 'silent',
});
const module = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
export const {
  MANUAL_TRANSACTION_WRITE_COMMANDS,
  manualTransactionUnavailableReason,
  manualTransactionStatusText,
} = module;
const namespaceBundle = await build({
  entryPoints: [fileURLToPath(new URL('../../src/core/namespace-label.ts', import.meta.url))],
  bundle: true, write: false, platform: 'node', format: 'esm', logLevel: 'silent',
});
const namespaceModule = await import(`data:text/javascript;base64,${Buffer.from(namespaceBundle.outputFiles[0].text).toString('base64')}`);
export const transactionFixtureBindings = {
  ...namespaceModule,
  MANUAL_TRANSACTION_WRITE_COMMANDS,
  manualTransactionUnavailableReason,
  manualTransactionStatusText,
};

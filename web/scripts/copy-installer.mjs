import { copyFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
const root = new URL('../../install.sh', import.meta.url);
const dest = new URL('../public/install.sh', import.meta.url);
copyFileSync(fileURLToPath(root), fileURLToPath(dest));

import fs from 'node:fs';
import path from 'node:path';

const cargoTomlPath = path.resolve(process.cwd(), '../Cargo.toml');
const cargoToml = fs.readFileSync(cargoTomlPath, 'utf-8');
const match = cargoToml.match(/\[workspace\.package\][\s\S]*?^\s*version\s*=\s*"([^"]+)"/m);

export const FORGE_VERSION = match ? match[1] : '0.4.0';
export const FORGE_TAG = `v${FORGE_VERSION}`;

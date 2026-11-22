#!/usr/bin/env node
// Copies the pinned third-party browser libraries from `node_modules` into
// `static/vendor/`, which is what the Desk ships and the server embeds into
// the binary (`ferris-web/src/vendor_assets.rs`). Run as `npm run vendor`
// after changing the pinned versions in `package.json`.
//
// Only the minified runtime files are copied (plus the leaflet image assets
// their CSS/JS reference with relative `images/…` paths) — not the npm
// packages' tests, TypeScript sources or source maps. The layout is flattened
// (no `dist/` level) so the output does not trip the repo's `dist/` gitignore
// rule; keep `images/` next to each CSS, whose `url(images/…)` is relative.
import { copyFileSync, mkdirSync, readdirSync, existsSync, statSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const webRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const nm = join(webRoot, "node_modules");
const out = join(webRoot, "static", "vendor");

/// (package, [relative source paths inside the package], flattened destination)
const FILES = {
	leaflet: {
		files: ["dist/leaflet.js", "dist/leaflet.css", "dist/images"],
	},
	"leaflet-draw": {
		files: ["dist/leaflet.draw.js", "dist/leaflet.draw.css", "dist/images"],
	},
	jsbarcode: {
		files: ["dist/JsBarcode.all.min.js"],
	},
	quill: {
		files: ["dist/quill.js", "dist/quill.core.css", "dist/quill.snow.css"],
	},
	marked: {
		files: ["marked.min.js"],
	},
};

/// The license file each package ships at its root, when present. Vendored
/// code keeps its licence text next to it.
const LICENSES = {
	leaflet: "LICENSE",
	"leaflet-draw": null, // package.json declares MIT; no file shipped upstream
	jsbarcode: "MIT-LICENSE.txt",
	quill: "LICENSE",
	marked: "LICENSE.md",
};

function copyDir(from, to) {
	mkdirSync(to, { recursive: true });
	for (const entry of readdirSync(from, { withFileTypes: true })) {
		const src = join(from, entry.name);
		const dst = join(to, entry.name);
		if (entry.isDirectory()) copyDir(src, dst);
		else copyFileSync(src, dst);
	}
}

for (const [pkg, { files }] of Object.entries(FILES)) {
	const pkgRoot = join(nm, pkg);
	if (!existsSync(pkgRoot)) {
		console.error(`missing node_modules/${pkg} — run \`npm ci\` first`);
		process.exit(1);
	}
	const destRoot = join(out, pkg);
	mkdirSync(destRoot, { recursive: true });
	for (const rel of files) {
		const src = join(pkgRoot, rel);
		// Flatten: `dist/leaflet.js` lands as `leaflet/leaflet.js`.
		const dst = join(destRoot, rel.replace(/^dist\//, ""));
		if (!existsSync(src)) continue;
		if (statSync(src).isDirectory()) {
			copyDir(src, dst);
			console.log(`vendor: ${pkg}/${rel}/ (dir)`);
		} else {
			mkdirSync(dirname(dst), { recursive: true });
			copyFileSync(src, dst);
			console.log(`vendor: ${pkg}/${rel}`);
		}
	}
	const license = LICENSES[pkg];
	if (license && existsSync(join(pkgRoot, license))) {
		copyFileSync(join(pkgRoot, license), join(destRoot, "LICENSE"));
		console.log(`vendor: ${pkg}/LICENSE`);
	}
}
console.log("vendor: done → static/vendor/");

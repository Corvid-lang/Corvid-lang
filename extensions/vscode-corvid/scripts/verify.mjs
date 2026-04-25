import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const jsonFiles = [
  "package.json",
  "language-configuration.json",
  "syntaxes/corvid.tmLanguage.json",
  "snippets/corvid.code-snippets"
];

for (const file of jsonFiles) {
  JSON.parse(readFileSync(resolve(root, file), "utf8"));
}

execFileSync(process.execPath, ["--check", resolve(root, "src/extension.js")], {
  stdio: "inherit"
});

console.log("corvid-vscode extension verification passed");

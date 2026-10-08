import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { readdir, writeFile } from "node:fs/promises";
import { basename, join, relative, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

const manifestName = "SHA256SUMS.txt";
const distributableSuffixes = [
  ".AppImage",
  ".deb",
  ".dmg",
  ".exe",
  ".msi",
  ".rpm",
  ".tar.gz",
  ".zip",
];

/**
 * Writes a deterministic SHA-256 manifest for every regular bundle artifact.
 * Paths are relative and normalized to forward slashes so a public checksum
 * file never discloses a CI runner path and remains identical across hosts.
 */
export async function writeBundleChecksums(directory) {
  const root = resolve(directory);
  const files = await collectRegularFiles(root, root);
  if (files.length === 0) {
    throw new Error(`no distributable bundle artifacts found in ${root}`);
  }
  const lines = [];
  for (const file of files) {
    const digest = await sha256(file.absolutePath);
    lines.push(`${digest}  ${file.relativePath}`);
  }
  const destination = join(root, manifestName);
  await writeFile(destination, `${lines.join("\n")}\n`, {
    encoding: "utf8",
    flag: "w",
  });
  return destination;
}

async function collectRegularFiles(root, directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const absolutePath = join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await collectRegularFiles(root, absolutePath)));
      continue;
    }
    if (
      !entry.isFile() ||
      basename(absolutePath) === manifestName ||
      !distributableSuffixes.some((suffix) => entry.name.endsWith(suffix))
    )
      continue;
    files.push({
      absolutePath,
      relativePath: relative(root, absolutePath).split(sep).join("/"),
    });
  }
  return files.sort((left, right) =>
    left.relativePath.localeCompare(right.relativePath, "en"),
  );
}

function sha256(path) {
  return new Promise((resolveDigest, reject) => {
    const hash = createHash("sha256");
    const source = createReadStream(path);
    source.on("error", reject);
    hash.on("error", reject);
    source.on("data", (chunk) => hash.update(chunk));
    source.on("end", () => resolveDigest(hash.digest("hex")));
  });
}

const invokedPath = process.argv[1];
if (
  invokedPath &&
  import.meta.url === pathToFileURL(resolve(invokedPath)).href
) {
  const directory = process.argv[2];
  if (!directory) {
    throw new Error("usage: write-bundle-checksums.mjs <bundle-directory>");
  }
  await writeBundleChecksums(directory);
}

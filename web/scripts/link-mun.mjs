// Mün is not published yet: link its workspace packages from a local checkout.
// Override the checkout location with MUN_PATH (default: ../../Mun next to Yeet).
import { existsSync, mkdirSync, rmSync, symlinkSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const web = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const mun = resolve(process.env.MUN_PATH ?? resolve(web, '../../Mun'))
const packages = ['core', 'web', 'vite', 'compiler', 'animation', 'execution']

if (!existsSync(resolve(mun, 'packages/core/dist'))) {
  console.error(`Mün checkout not built at ${mun}. Run \`pnpm build\` there or set MUN_PATH.`)
  process.exit(1)
}
mkdirSync(resolve(web, 'node_modules/@mun'), { recursive: true })
for (const name of packages) {
  const link = resolve(web, 'node_modules/@mun', name)
  rmSync(link, { recursive: true, force: true })
  symlinkSync(resolve(mun, 'packages', name), link, 'dir')
}

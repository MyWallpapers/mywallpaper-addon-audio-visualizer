import { defineConfig, type Plugin } from 'vite'

const DISTRIBUTION_ENTRY = 'assets/addon.js'
const DEVELOPMENT_ENTRY_PATH = '/dist/' + DISTRIBUTION_ENTRY
const SOURCE_ENTRY_PATH = '/src/main.ts'

function myWallpaperAddon(): Plugin {
  return {
    name: 'mywallpaper-addon',
    config() {
      return {
        base: './',
        server: { cors: true },
        build: {
          rollupOptions: {
            preserveEntrySignatures: 'strict',
            input: 'src/main.ts',
            output: {
              entryFileNames: DISTRIBUTION_ENTRY,
              chunkFileNames: 'assets/chunk-[name]-[hash].js',
              assetFileNames: 'assets/[name]-[hash][extname]',
            },
          },
        },
      }
    },
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        let pathname: string
        try {
          pathname = new URL(request.url ?? '/', 'http://localhost').pathname
        } catch {
          next()
          return
        }
        if (pathname !== DEVELOPMENT_ENTRY_PATH) {
          next()
          return
        }

        response.statusCode = 200
        response.setHeader('Content-Type', 'text/javascript; charset=utf-8')
        response.setHeader('Cache-Control', 'no-store')
        response.setHeader('Access-Control-Allow-Origin', '*')
        response.setHeader('Cross-Origin-Resource-Policy', 'cross-origin')
        response.end(request.method === 'HEAD'
          ? undefined
          : 'export { mount } from ' + JSON.stringify(SOURCE_ENTRY_PATH) + '\n')
      })
    },
    generateBundle(_options, bundle) {
      const entry = bundle[DISTRIBUTION_ENTRY]
      if (!entry || entry.type !== 'chunk' || !entry.exports.includes('mount')) {
        this.error('The production add-on entry must export mount.')
      }
      const styles = Object.values(bundle)
        .filter((output) => output.type === 'asset' && output.fileName.endsWith('.css'))
        .map((output) => output.fileName.split('/').at(-1))
        .filter((fileName): fileName is string => fileName !== undefined)
      const stylesheetBootstrap = styles.map((fileName) => [
        '{',
        '  const href = new URL(' + JSON.stringify('./' + fileName) + ', import.meta.url).href',
        '  let link = [...document.querySelectorAll(\'link[rel~="stylesheet"]\')]',
        '    .find((candidate) => candidate.href === href)',
        '  if (!link) {',
        '    link = document.createElement(\'link\')',
        '    link.rel = \'stylesheet\'',
        '    link.href = href',
        '    document.head.append(link)',
        '  }',
        '  if (!link.sheet) await new Promise((resolve, reject) => {',
        '    link.addEventListener(\'load\', resolve, { once: true })',
        '    link.addEventListener(\'error\', () => reject(new Error(\'Failed to load add-on stylesheet: \' + href)), { once: true })',
        '  })',
        '}',
      ].join('\n')).join('\n')
      entry.code = stylesheetBootstrap + '\n' + entry.code
    },
  }
}

export default defineConfig(({ command }) => ({
  // Development-only runtime output must never become a release asset.
  publicDir: command === 'build' ? false : 'public',
  plugins: [myWallpaperAddon()],
  server: {
    host: '127.0.0.1',
    port: 5181,
    strictPort: true,
  },
}))

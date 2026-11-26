import tailwindcss from '@tailwindcss/vite'
import { mkdirSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { defineConfig } from 'wxt'

// Resolve @page-agent/* packages from the local packages directory.
// The packages ship pre-built dist files — alias to those directly so Vite
// doesn't try to rebuild them from source.
const PAGE_AGENT_ROOT = resolve(__dirname, 'packages')

const chromeProfile = '.wxt/chrome-data'
mkdirSync(chromeProfile, { recursive: true })

const pkg = JSON.parse(readFileSync('./package.json', 'utf-8'))
const stableExtensionKey =
  process.env.ALLTERNIT_EXTENSION_KEY ||
  'MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAvha6YaRIJfHgEURtE2+5O/WgDNrE2VysLDd0UZN/5LPcFLYVHmTYwV2w1mNiG7tKfVQRy9Y31aOdHHBbXq44jNOP8JoJVR91ZxKMqFFj7JrrPJZLbSceHB6tnE3sTU+sY0Xhi+JUVuLYHmGstL+pPydyhW6BON42NmHh7LBnhXU6p3HaCRqR3Km7wJpeNjknIKUhP9K7GcKPJeTaBuFbz+z+dPSavQS1VyJvKdHMClY8/GwWRFcxTw5r85+IqFrO1SATWlGySFPaJYKLO5ElkYNMaLHJnl+VoogvnOYKegUjaoFswrfXk6SHfOzExn0rjbINYcYmM8Fwn4kqMF7/cwIDAQAB'

export default defineConfig({
  srcDir: 'src',
  modules: ['@wxt-dev/module-react'],
  webExt: {
    chromiumProfile: chromeProfile,
    keepProfileChanges: true,
    chromiumArgs: ['--hide-crash-restore-bubble', '--remote-debugging-port=9222'],
  },
  vite: () => ({
    plugins: [tailwindcss()],
    resolve: {
      alias: {
        react: resolve(__dirname, 'node_modules/react'),
        'react-dom': resolve(__dirname, 'node_modules/react-dom'),
        'react/jsx-runtime': resolve(__dirname, 'node_modules/react/jsx-runtime.js'),
        '@page-agent/core': resolve(PAGE_AGENT_ROOT, 'core/dist/esm/page-agent-core.js'),
        '@page-agent/llms': resolve(PAGE_AGENT_ROOT, 'llms/dist/lib/page-agent-llms.js'),
        '@page-agent/page-controller': resolve(PAGE_AGENT_ROOT, 'page-controller/dist/lib/page-controller.js'),
        '@page-agent/ui': resolve(PAGE_AGENT_ROOT, 'ui/dist/lib/page-agent-ui.js'),
      },
    },
    define: {
      __VERSION__: JSON.stringify(pkg.version),
    },
    optimizeDeps: { force: true },
    build: {
      minify: false,
      chunkSizeWarningLimit: 2000,
      rollupOptions: {
        onwarn(message, handler) {
          if (message.code === 'EVAL') return
          handler(message)
        },
      },
    },
  }),
  zip: {
    artifactTemplate: 'allternit-ext-{{version}}-{{browser}}.zip',
  },
  manifest: {
    name: 'Allternit Extension',
    description: 'Unified browser automation and page agent for Allternit',
    version: pkg.version,
    key: stableExtensionKey,
    permissions: [
      // browser-agent capabilities
      'activeTab',
      'scripting',
      'webNavigation',
      // page-agent capabilities
      'tabGroups',
      'sidePanel',
      // shared
      'storage',
      'tabs',
      'nativeMessaging',
      // memory: browser history ingestion
      'history',
      // polish features
      'notifications',
      'downloads',
      'contextMenus',
      // api-capture: full network recording via Chrome DevTools Protocol
      'debugger',
      'webRequest',
    ],
    commands: {
      'open-sidepanel': {
        suggested_key: { default: 'Alt+Shift+A' },
        description: 'Open Allternit sidepanel',
      },
      'capture-page': {
        suggested_key: { default: 'Alt+Shift+C' },
        description: 'Capture current page for HTML to Figma',
      },
      'quick-task': {
        suggested_key: { default: 'Alt+Shift+T' },
        description: 'Open quick task input',
      },
    },
    host_permissions: ['<all_urls>'],
    background: {
      service_worker: 'background.js',
    },
    action: {
      default_title: 'Allternit Extension',
    },
    side_panel: {
      default_path: 'sidepanel.html',
    },
    web_accessible_resources: [
      {
        resources: ['main-world.js', 'injected.js'],
        matches: ['*://*/*'],
      },
    ],
    // Allow Allternit Desktop app and cloud to communicate via externally_connectable
    externally_connectable: {
      matches: ['http://localhost:*/*', 'https://*.allternit.com/*'],
    },
  },
})

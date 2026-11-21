// The browser globals the UI modules read when they load (api.js, store.js),
// for tests in Node. There is no DOM: a test replaces
// `window.__TAURI__.core.invoke` to answer backend calls.
globalThis.navigator ??= { userAgent: 'node' };
globalThis.window = {
  __TAURI__: {
    core: { invoke: async (cmd) => { throw new Error(`no backend for ${cmd}`); } },
    event: { listen: async () => () => {} },
  },
  addEventListener() {},
};
globalThis.document = { documentElement: { lang: 'en' }, addEventListener() {} };
// in memory (Node's own needs --localstorage-file and warns without it)
const saved = new Map();
Object.defineProperty(globalThis, 'localStorage', {
  configurable: true,
  value: { getItem: (k) => saved.get(k) ?? null, setItem: (k, v) => saved.set(k, String(v)), removeItem: (k) => saved.delete(k) },
});

// Settings pages whose backend a build may not have: Tauri rejects a call
// to a command that isn't registered with "Command <name> not found".
export const notRegistered = (e) => /^Command \S+ not found$/.test((e && e.message) || String(e || ''));

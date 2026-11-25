// What the app shows of an MCP server or tool besides its name: the server's icon (MCP 2025-11-25
// icons, kept to data: and https: images by mcp.rs icons_of), a tool's title and description, and
// the hints its server gives about it (read-only, destructive). Used by the server cards in
// Settings → Developer and by the permission card of a tool call.
import { t } from './i18n.js';
import { esc } from './util.js';
import { icon } from './icons.js';
import { store } from './store.js';

/** The icon of MCP `icons` to show on a `theme` ("light"/"dark") background: one made for it, else one for any theme, else any; null when none. */
export function pickIcon(icons, theme) {
  const list = Array.isArray(icons) ? icons.filter((i) => i && typeof i.src === 'string') : [];
  return list.find((i) => i.theme === theme) || list.find((i) => !i.theme) || list[0] || null;
}

/** `icons` as an image of `size` px for the app's theme, else the icons.js icon `fallback`;
 *  one with a picture for each theme keeps both, and follows a theme change (below). */
export function iconHtml(icons, size, fallback) {
  const theme = document.documentElement.dataset?.theme === 'dark' ? 'dark' : 'light';
  const i = pickIcon(icons, theme);
  if (!i) return icon(fallback, size);
  const [light, dark] = [pickIcon(icons, 'light'), pickIcon(icons, 'dark')];
  const both = light.src !== dark.src ? ` data-light="${esc(light.src)}" data-dark="${esc(dark.src)}"` : '';
  return `<img class="mcp-icon" src="${esc(i.src)}" width="${size}" height="${size}" alt="" referrerpolicy="no-referrer" decoding="async"${both}>`;
}

// (not in the Node tests, which have no DOM)
if (typeof MutationObserver !== 'undefined') {
  new MutationObserver(() => {
    const theme = document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light';
    document.querySelectorAll('img.mcp-icon[data-dark]').forEach((img) => { img.src = img.dataset[theme]; });
  }).observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
}

/** The badges of what `tool` (an MCP tool of mcp_status) says about itself. */
export function toolHintsHtml(tool) {
  const hints = [];
  if (tool.read_only) hints.push(`<span class="tool-hint">${esc(t('tool_hint_read_only'))}</span>`);
  if (tool.destructive) hints.push(`<span class="tool-hint danger">${esc(t('tool_hint_destructive'))}</span>`);
  return hints.join('');
}

/** A tool row's label: its title (else its name), its icon and hints; with a description or a title,
 *  a disclosure that shows the description and the tool's own name (also on hover). */
export function toolLabelHtml(tool) {
  const shown = tool.title || tool.name;
  const head = `${tool.icons?.length ? iconHtml(tool.icons, 14, 'wrench') : ''}<span class="nm${tool.title ? ' titled' : ''}">${esc(shown)}</span>${toolHintsHtml(tool)}`;
  if (!tool.description && shown === tool.name) return `<div class="tool-info">${head}</div>`;
  const about = `${shown !== tool.name ? `<span class="mono">${esc(tool.name)}</span> ` : ''}${esc(tool.description || '')}`;
  return `<details class="tool-info"><summary title="${esc(tool.description || tool.name)}">${head}${icon('chevDown', 12)}</summary><div class="tool-desc small muted">${about}</div></details>`;
}

/** Tool `name` of server `server` as the app last listed it (store.mcpServers), or null. */
export function findTool(server, name) {
  return store.mcpServers?.find((s) => s.name === server)?.tools?.find((x) => x.name === name) || null;
}

/** The parts of the permission card for MCP tool request `p` ({server, tool_name, tool}): its
 *  title (HTML), the line under it (text) and the tool's description (HTML, or ''). */
export function permissionInfo(p) {
  const tool = findTool(p.server, p.tool_name);
  const name = p.tool_name || p.tool;
  const shown = tool?.title || name;
  const serverIcons = store.mcpServers?.find((s) => s.name === p.server)?.server_info?.serverInfo?.icons;
  return {
    title: `${iconHtml(serverIcons, 17, 'plug')} ${t('wants_to_use')} <span class="${shown === name ? 'mono' : 'perm-tool'}">${esc(shown)}</span>${tool ? toolHintsHtml(tool) : ''}`,
    desc: `${t('from_server', { server: p.server })}${shown !== name ? ` · ${name}` : ''}`,
    about: tool?.description ? `<p class="perm-tool-desc" title="${esc(tool.description)}">${esc(tool.description)}</p>` : '',
  };
}

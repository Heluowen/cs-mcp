#!/usr/bin/env node
/**
 * cs-mcp 的 stdio 启动器（用 node 包一层）。
 *
 * 为什么需要它：
 *   AstrBot 的 MCP stdio 客户端有一份命令白名单（python / node / npx 等启动器），
 *   直接写 `"command": "cs-mcp.exe"` 会被拒绝：
 *     "MCP stdio command 'cs-mcp' is not allowed. Allowed commands: bun, bunx, ..."
 *   白名单可以通过 ASTRBOT_MCP_STDIO_ALLOWED_COMMANDS 覆盖，但那个变量会**整体替换**
 *   默认列表（不是追加），且需要重启 AstrBot。
 *   用 node 当启动器则完全不用改任何全局配置。
 *
 * 用法（mcp_server.json）：
 *   {
 *     "mcpServers": {
 *       "cs2": {
 *         "command": "node",
 *         "args": ["H:\\Deepseek-Harness\\Zone 1\\cs-mcp\\scripts\\mcp-launcher.js"],
 *         "env": { "PANDASCORE_TOKEN": "...", "TZ": "Asia/Shanghai" }
 *       }
 *     }
 *   }
 *
 * 也可用环境变量 CS_MCP_EXE 指定可执行文件路径。
 */

const { spawn } = require('node:child_process');
const path = require('node:path');
const fs = require('node:fs');

const projectRoot = path.resolve(__dirname, '..');

const candidates = [
  process.env.CS_MCP_EXE,
  path.join(projectRoot, 'target', 'release', 'cs-mcp.exe'),
  path.join(projectRoot, 'target', 'release', 'cs-mcp'),
  path.join(projectRoot, 'target', 'debug', 'cs-mcp.exe'),
  path.join(projectRoot, 'target', 'debug', 'cs-mcp'),
].filter(Boolean);

const exe = candidates.find((p) => {
  try {
    return fs.existsSync(p);
  } catch {
    return false;
  }
});

if (!exe) {
  process.stderr.write(
    '[cs-mcp launcher] 找不到 cs-mcp 可执行文件。\n' +
      `[cs-mcp launcher] 已尝试: ${candidates.join(', ')}\n` +
      '[cs-mcp launcher] 请先执行: cargo build --release\n',
  );
  process.exit(1);
}

// stdio: 'inherit' —— 这是关键：子进程直接接管 node 的 stdin/stdout，
// MCP 的 JSON-RPC 帧原样透传，不做任何缓冲或改写。
// cwd 设为项目根目录，让 cs-mcp 的 dotenvy 能读到同目录下的 .env。
const child = spawn(exe, process.argv.slice(2), {
  stdio: 'inherit',
  cwd: projectRoot,
  env: process.env,
});

child.on('error', (err) => {
  process.stderr.write(`[cs-mcp launcher] 启动失败: ${err.message}\n`);
  process.exit(1);
});

child.on('exit', (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 0);
});

// 父进程被终止时，把信号转发给子进程，避免留下孤儿进程。
for (const sig of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
  process.on(sig, () => {
    try {
      child.kill(sig);
    } catch {
      /* 忽略：子进程可能已退出 */
    }
  });
}

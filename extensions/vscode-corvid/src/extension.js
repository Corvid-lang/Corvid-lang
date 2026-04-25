"use strict";

const fs = require("fs");
const path = require("path");
const vscode = require("vscode");
const {
  LanguageClient,
  TransportKind,
  Trace,
} = require("vscode-languageclient/node");

let client = null;
let statusItem = null;
let outputChannel = null;

async function activate(context) {
  outputChannel = vscode.window.createOutputChannel("Corvid Language Server");
  statusItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 75);
  statusItem.command = "corvid.showLanguageServerLog";
  context.subscriptions.push(outputChannel, statusItem);

  context.subscriptions.push(
    vscode.commands.registerCommand("corvid.restartLanguageServer", async () => {
      await restartClient(context);
    }),
    vscode.commands.registerCommand("corvid.showLanguageServerLog", () => {
      outputChannel.show();
    })
  );

  await startClient(context);
}

async function deactivate() {
  await stopClient();
}

async function restartClient(context) {
  outputChannel.appendLine("[corvid] restarting language server");
  await stopClient();
  await startClient(context);
}

async function startClient(context) {
  const serverPath = resolveServerPath(context);
  const serverOptions = {
    command: serverPath,
    transport: TransportKind.stdio,
    options: {
      cwd: workspaceRoot() || context.extensionPath,
      env: { ...process.env },
    },
  };
  const clientOptions = {
    documentSelector: [{ scheme: "file", language: "corvid" }],
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher("**/*.cor"),
    },
    outputChannel,
  };

  client = new LanguageClient(
    "corvid-lsp",
    "Corvid Language Server",
    serverOptions,
    clientOptions
  );
  client.setTrace(traceSetting());
  client.onDidChangeState((event) => {
    setStatus(event.newState);
  });
  setStatus(undefined, "starting");
  context.subscriptions.push(client.start());
  outputChannel.appendLine(`[corvid] using language server: ${serverPath}`);
}

async function stopClient() {
  if (!client) {
    return;
  }
  const current = client;
  client = null;
  setStatus(undefined, "stopping");
  await current.stop();
  setStatus(undefined, "stopped");
}

function resolveServerPath(context) {
  const configured = vscode.workspace
    .getConfiguration("corvid")
    .get("lsp.path", "");
  const candidates = [
    configured,
    process.env.CORVID_LSP_PATH,
    repoBinary(context, "debug"),
    repoBinary(context, "release"),
    "corvid-lsp",
  ].filter(Boolean);

  for (const candidate of candidates) {
    if (candidate === "corvid-lsp" || fs.existsSync(candidate)) {
      return candidate;
    }
  }
  return "corvid-lsp";
}

function repoBinary(context, profile) {
  const exe = process.platform === "win32" ? "corvid-lsp.exe" : "corvid-lsp";
  return path.resolve(context.extensionPath, "..", "..", "target", profile, exe);
}

function traceSetting() {
  const value = vscode.workspace
    .getConfiguration("corvid")
    .get("lsp.trace.server", "off");
  switch (value) {
    case "messages":
      return Trace.Messages;
    case "verbose":
      return Trace.Verbose;
    default:
      return Trace.Off;
  }
}

function setStatus(state, label) {
  if (!statusItem) {
    return;
  }
  const text = label || String(state || "idle").toLowerCase();
  statusItem.text = `Corvid LSP: ${text}`;
  statusItem.tooltip =
    "Corvid LSP provides diagnostics, hover, completion, navigation, rename, and workspace symbols.";
  statusItem.show();
}

function workspaceRoot() {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

module.exports = {
  activate,
  deactivate,
  // Exported for the verification script.
  _resolveServerPathForTests: resolveServerPath,
};

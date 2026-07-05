// A tiny extension that exercises the first-slice API: on activation it pops an info
// message, registers a command, and registers a hover provider that reports the line.
'use strict';

const vscode = require('vscode');

function activate(context) {
  vscode.window.showInformationMessage('hello from the aether sample extension');

  context.subscriptions.push(
    vscode.commands.registerCommand('aether.sample.hello', () =>
      vscode.window.showInformationMessage('sample command ran')
    )
  );

  context.subscriptions.push(
    vscode.languages.registerHoverProvider(['plaintext', 'rust', '*'], {
      provideHover(document, position) {
        const md = new vscode.MarkdownString();
        md.appendCodeblock(`aether sample`, 'text');
        md.appendMarkdown(`\nHovered **line ${position.line}, col ${position.character}**.`);
        return new vscode.Hover(md, new vscode.Range(position.line, position.character, position.line, position.character + 1));
      },
    })
  );
}

function deactivate() {}

module.exports = { activate, deactivate };

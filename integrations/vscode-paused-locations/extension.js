'use strict';

const vscode = require('vscode');
const { register } = require('./command');

exports.activate = context => { context.subscriptions.push(register(vscode)); };

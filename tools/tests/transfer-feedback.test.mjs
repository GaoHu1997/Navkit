import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';

// Execute the real function, not a copied implementation. AST extraction avoids
// importing the application/IPC stack or depending on adjacent dialog functions.
const sourcePath = new URL('../../src/dialogs/SyncDialog.ts', import.meta.url);
const source = fs.readFileSync(sourcePath, 'utf8');
const sourceFile = ts.createSourceFile(sourcePath.pathname, source, ts.ScriptTarget.ES2021, true);
const declaration = sourceFile.statements.find(node =>
  ts.isFunctionDeclaration(node) && node.name?.text === 'taskFeedback');
assert.ok(declaration, 'SyncDialog.ts must export the real taskFeedback function');
const compiled = ts.transpileModule(
  declaration.getText(sourceFile).replace(/^export\s+/, '') + '\nglobalThis.taskFeedback = taskFeedback;',
  { compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.None } },
).outputText;

class Element {
  constructor(tag = 'div') {
    this.tagName = tag;
    this.children = [];
    this.style = {};
    this.attributes = new Map();
    this.listeners = new Map();
    this.disabled = false;
    this.hidden = false;
    this.writes = [];
    this._text = '';
  }
  set textContent(value) { this._text = value; this.writes.push(['textContent', value]); }
  get textContent() { return this._text; }
  set max(value) { this._max = value; this.writes.push(['max', value]); }
  get max() { return this._max; }
  set value(value) { this._value = value; this.writes.push(['value', value]); }
  get value() { return this._value; }
  get firstChild() { return this.children[0] ?? null; }
  append(...children) { this.children.push(...children); }
  insertBefore(child, before) {
    const index = this.children.indexOf(before);
    if (index < 0) this.children.push(child); else this.children.splice(index, 0, child);
  }
  setAttribute(name, value) { this.attributes.set(name, value); }
  removeAttribute(name) { this.attributes.delete(name); }
  addEventListener(name, callback) { this.listeners.set(name, callback); }
  querySelectorAll() { return this.children.filter(c =>
    ['input', 'select', 'textarea', 'button'].includes(c.tagName) && !c.disabled); }
}

function harness() {
  const intervals = new Map();
  const requests = [];
  const cancellations = [];
  let intervalId = 0;
  let uuid = 0;
  let onClose;
  const document = { createElement: tag => new Element(tag) };
  const body = new Element();
  const footer = new Element();
  const control = new Element('input');
  body.append(control);
  const status = new Element();
  const dlg = { body, footer, onClose(callback) { onClose = callback; } };
  const context = vm.createContext({
    document,
    crypto: { randomUUID: () => `task-${++uuid}` },
    setInterval(callback, delay) {
      assert.equal(delay, 250);
      const id = ++intervalId;
      intervals.set(id, callback);
      return id;
    },
    clearInterval(id) { intervals.delete(id); },
    transferTasks: {
      progress(taskId) {
        let resolve;
        let reject;
        const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
        requests.push({ taskId, resolve, reject });
        return promise;
      },
      cancel(taskId) { cancellations.push(taskId); return Promise.resolve(); },
    },
  });
  vm.runInContext(compiled, context, { filename: sourcePath.pathname });
  const feedback = context.taskFeedback(dlg, status);
  const progress = body.children.find(c => c.tagName === 'progress');
  const cancel = footer.children.find(c => c.tagName === 'button');
  return {
    feedback, status, progress, cancel, control, body, intervals, requests, cancellations,
    close() { assert.equal(typeof onClose, 'function'); onClose(); },
    poll() {
      assert.equal(intervals.size, 1, 'exactly one current poll timer must exist');
      const callback = intervals.values().next().value;
      const pending = callback();
      const request = requests[requests.length - 1];
      assert.ok(request, 'poll must call the mocked async progress API');
      return { pending, request };
    },
  };
}

function response(taskId, overrides = {}) {
  return {
    task_id: taskId, status: 'running', phase: 'write',
    processed: 1, total: 4, message: '当前任务正在写入', ...overrides,
  };
}

function writesOf(h) {
  return { status: [...h.status.writes], progress: [...h.progress.writes] };
}

test('an async poll resolving after finish cannot overwrite the final status or progress', async () => {
  const h = harness();
  const taskId = h.feedback.begin();
  const { pending, request } = h.poll();
  assert.equal(request.taskId, taskId);
  h.feedback.finish();
  h.status.textContent = '同步完成：4 行';
  const finalWrites = writesOf(h);
  request.resolve(response(taskId, { status: 'completed', message: '过期响应', processed: 4 }));
  await pending;
  assert.equal(h.status.textContent, '同步完成：4 行');
  assert.deepEqual(writesOf(h), finalWrites, 'stale polling must perform no status/progress writes');
  assert.equal(h.intervals.size, 0);
  assert.equal(h.progress.hidden, true);
  assert.equal(h.cancel.disabled, true);
  assert.equal(h.control.disabled, false);
  assert.equal(h.body.attributes.has('aria-busy'), false);
});

test('an async poll resolving after close does not write status or progress', async () => {
  const h = harness();
  const taskId = h.feedback.begin();
  const { pending, request } = h.poll();
  const beforeClose = writesOf(h);
  h.close();
  assert.equal(h.intervals.size, 0);
  assert.deepEqual(h.cancellations, [taskId]);
  request.resolve(response(taskId, { message: '关闭后的过期响应' }));
  await pending;
  assert.deepEqual(writesOf(h), beforeClose, 'closed dialogs must not receive late poll mutations');
});

test('begin invalidates an old response without overwriting or stopping the new task', async () => {
  const h = harness();
  const oldId = h.feedback.begin();
  const oldPoll = h.poll();
  const newId = h.feedback.begin();
  assert.notEqual(newId, oldId);
  assert.equal(h.intervals.size, 1, 'begin must replace the previous timer');
  h.status.textContent = '新任务启动中';
  const newWrites = writesOf(h);
  oldPoll.request.resolve(response(oldId, { status: 'completed', message: '旧任务已完成', processed: 4 }));
  await oldPoll.pending;
  assert.deepEqual(writesOf(h), newWrites);
  assert.equal(h.status.textContent, '新任务启动中');
  assert.equal(h.intervals.size, 1, 'a stale terminal response must not clear the new timer');
  const currentPoll = h.poll();
  assert.equal(currentPoll.request.taskId, newId);
  currentPoll.request.resolve(response(newId, { message: '新任务写入中', processed: 2, total: 6 }));
  await currentPoll.pending;
  assert.equal(h.status.textContent, '新任务写入中\n2 / 6 行');
  assert.equal(h.progress.value, 2);
  assert.equal(h.progress.max, 6);
  h.feedback.finish();
});

test('finish followed by begin isolates an old rejection and allows the new task to poll', async () => {
  const h = harness();
  const oldId = h.feedback.begin();
  const oldPoll = h.poll();
  h.feedback.finish();
  const newId = h.feedback.begin();
  assert.notEqual(newId, oldId);
  const newWrites = writesOf(h);
  oldPoll.request.reject(new Error('旧任务查询失败'));
  await oldPoll.pending;
  assert.deepEqual(writesOf(h), newWrites);
  assert.equal(h.intervals.size, 1);
  const currentPoll = h.poll();
  assert.equal(currentPoll.request.taskId, newId);
  currentPoll.request.resolve(response(newId, { status: 'completed', message: '新任务完成', processed: 4 }));
  await currentPoll.pending;
  assert.equal(h.status.textContent, '新任务完成\n4 / 4 行');
  assert.equal(h.intervals.size, 0);
  h.feedback.finish();
});

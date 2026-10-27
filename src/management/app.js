'use strict';

const byId = (id) => document.getElementById(id);
let connected = true;
let pending = false;
let previousStatus;
let tableScroll = 0;
const events = [];

function notice(message, kind = 'info') {
  const element = byId('operation');
  element.textContent = message;
  element.className = `notice ${kind}`;
  element.hidden = !message;
}
function addEvent(message) {
  events.unshift({ message, date: new Date() });
  events.splice(20);
  byId('events').replaceChildren(...events.map((event) => {
    const item = document.createElement('li');
    const time = document.createElement('time');
    time.dateTime = event.date.toISOString();
    time.textContent = event.date.toLocaleTimeString('ja-JP');
    item.append(time, document.createTextNode(event.message));
    return item;
  }));
}
function actions() {
  document.querySelectorAll('[data-action]').forEach((button) => {
    button.disabled = pending || !connected || button.dataset.available !== 'true';
  });
}
function received() {
  // htmx normally binds new controls during settling, after afterSwap fires.
  // Bind them before actions() makes the refreshed controls available to users.
  htmx.process(byId('dashboard'));
  if (!connected) addEvent('サーバーとの接続が回復しました。');
  connected = true;
  byId('connection').textContent = '接続中';
  byId('connection').className = 'badge good';
  byId('connection-error').hidden = true;
  byId('updated').textContent = `最終取得：${new Date().toLocaleTimeString('ja-JP')}`;
  const status = byId('dashboard-content').dataset.event;
  if (status !== previousStatus) { addEvent(status); previousStatus = status; }
  actions();
}
function disconnected() {
  if (connected) addEvent('サーバーとの接続が切れました。');
  connected = false;
  byId('connection').textContent = '接続エラー';
  byId('connection').className = 'badge bad';
  byId('connection-error').hidden = false;
  byId('connection-error').textContent = '現在の状態を取得できません。lllmmmの起動を確認してください。';
  document.querySelectorAll('[data-live]').forEach((element) => { element.textContent = '未確認'; });
  actions();
}
function refresh() { htmx.trigger(document.body, 'refresh'); }

document.body.addEventListener('htmx:beforeRequest', (event) => {
  const request = event.detail.requestConfig;
  if (request.verb === 'get') {
    if (!byId('auto-refresh').checked && request.triggeringEvent?.type !== 'refresh') {
      event.preventDefault(); return;
    }
    byId('refresh').disabled = true;
  } else if (request.verb === 'post') {
    if (pending || !connected) { event.preventDefault(); return; }
    pending = true;
    const xhr = event.detail.xhr;
    // Polling can replace the initiating button before completion. The XHR
    // outlives that element, so completion must not rely on a bubbling DOM event.
    xhr.addEventListener('loadend', () => {
      pending = false;
      if (!(xhr.status >= 200 && xhr.status < 300)
          && !xhr.getResponseHeader('Content-Type')?.startsWith('text/html')) {
        const message = '操作の完了を確認できませんでした。状態を更新して結果を確認してください。';
        notice(message, 'error'); addEvent(message);
      }
      actions();
      refresh();
    }, { once: true });
    notice('操作を要求しています。先行する要求がある場合は完了を待ちます。');
    actions();
  }
});
document.body.addEventListener('htmx:beforeSwap', (event) => {
  const { xhr, target } = event.detail;
  if (target.id === 'dashboard') tableScroll = document.querySelector('.table-scroll').scrollLeft;
  // Error notices are rendered and escaped by Askama, just like successful results.
  if (target.id === 'operation' && xhr.getResponseHeader('Content-Type')?.startsWith('text/html')) {
    event.detail.shouldSwap = true;
  }
});
document.body.addEventListener('htmx:afterSwap', (event) => {
  if (event.detail.target.id === 'dashboard') {
    document.querySelector('.table-scroll').scrollLeft = tableScroll;
    received();
  }
  if (event.detail.target.id === 'operation') addEvent(byId('operation').textContent);
});
document.body.addEventListener('htmx:afterRequest', (event) => {
  const { requestConfig, successful } = event.detail;
  if (requestConfig.verb === 'get') {
    byId('refresh').disabled = false;
    if (!successful) disconnected();
  }
});
byId('refresh').addEventListener('click', refresh);
byId('auto-refresh').addEventListener('change', () => { if (byId('auto-refresh').checked) refresh(); });
const theme = byId('theme');
theme.value = document.documentElement.dataset.theme || 'system';
theme.addEventListener('change', () => {
  if (theme.value === 'system') delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme.value;
  try { localStorage.setItem('lllmmm-theme', theme.value); } catch {}
});
received();

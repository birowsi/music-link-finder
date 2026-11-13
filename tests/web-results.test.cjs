const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');

// Run the shipped controller without a browser or added dependencies. This
// covers its DOM order, selection, copying and stale-request behavior together.
class Element {
  constructor(tag = 'div') {
    this.tagName = tag;
    this.children = [];
    this.attributes = {};
    this.listeners = {};
    this.className = '';
    this.textContent = '';
    this.classList = {
      add: name => { this.className = [...new Set([...this.className.split(' '), name])].join(' ').trim(); },
      remove: name => { this.className = this.className.split(' ').filter(item => item !== name).join(' '); }
    };
  }
  append(...children) { this.children.push(...children); }
  replaceChildren(...children) { this.children = children; }
  setAttribute(key, value) { this.attributes[key] = value; }
  removeAttribute(key) { delete this.attributes[key]; }
  addEventListener(event, callback) { this.listeners[event] = callback; }
  querySelectorAll(selector) {
    const found = [];
    for (const child of this.children) {
      if (child.className.split(' ').includes(selector.slice(1))) found.push(child);
      found.push(...child.querySelectorAll(selector));
    }
    return found;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  focus() {}
  select() {}
  scrollIntoView() {}
}

const source = readFileSync(join(__dirname, '../web/app.js'), 'utf8');
const response = results => ({ ok: true, json: async () => ({ results }) });
const video = (id, kind, flags = {}) => ({
  title: `${kind} video`, channel: '채널', kind,
  url: `https://www.youtube.com/watch?v=${id.repeat(11)}`,
  is_fallback: false, is_recommendation: false, ...flags
});

function setup(fetch) {
  const elements = new Map();
  const get = selector => {
    if (!elements.has(selector)) elements.set(selector, new Element());
    return elements.get(selector);
  };
  const copied = [];
  const modeInputs = [new Element('input'), new Element('input')];
  const form = get('#search-form');
  form.querySelectorAll = () => modeInputs;
  const harness = { get, copied, mode: 'pronunciation' };
  form.requestSubmit = () => { harness.pendingSubmit = harness.submit(); };
  harness.submit = () => form.listeners.submit({ preventDefault() {} });
  vm.runInNewContext(source, {
    document: { querySelector: get, createElement: tag => new Element(tag) },
    FormData: class { get() { return harness.mode; } },
    AbortController, URLSearchParams, fetch,
    navigator: { clipboard: { writeText: async url => { copied.push(url); } } },
    window: { setTimeout() {}, matchMedia: () => ({ matches: true }) }
  });
  get('#song-query').value = '잇떼';
  harness.changeMode = mode => {
    harness.mode = mode;
    modeInputs[1].listeners.change();
    return harness.pendingSubmit;
  };
  harness.cards = () => get('#results').querySelectorAll('.video-card');
  harness.labels = () => harness.cards().map(card => card.querySelector('.video-channel').textContent);
  return harness;
}

test('general fallback precedes subtitles and recommendations in the DOM and selection', async () => {
  const general = video('a', 'general', { is_fallback: true });
  const subtitle = video('b', 'subtitle');
  const related = video('c', 'pronunciation', { is_recommendation: true });
  const app = setup(async () => response([general, subtitle, related]));
  await app.submit();
  assert.deepEqual(app.labels(), ['일반 결과  |  채널', '자막  |  채널', '관련 추천  |  채널']);
  assert.equal(app.get('#selected-open').href, general.url);
  assert.equal(app.get('#selected-kind').textContent, '일반 결과');
  assert.equal(app.cards()[0].attributes['aria-pressed'], 'true');
  assert.equal(app.copied.length, 0);
});

test('automatic copy follows the displayed first result when a subtitle is also present', async () => {
  const general = video('a', 'general', { is_fallback: true });
  const app = setup(async () => response([general, video('b', 'subtitle')]));
  app.get('#auto-copy').checked = true;
  await app.submit();
  assert.deepEqual(app.copied, [general.url]);
  assert.equal(app.get('#selected-open').href, app.copied[0]);
});

test('pronunciation and subtitle labels follow server evidence rather than the active tab', async () => {
  const subtitle = video('b', 'subtitle');
  const app = setup(async () => response([video('a', 'pronunciation'), subtitle, video('c', 'subtitle', { is_recommendation: true })]));
  await app.submit();
  assert.deepEqual(app.labels(), ['한글 발음  |  채널', '자막  |  채널', '관련 추천 · 자막  |  채널']);
  app.cards()[1].listeners.click();
  assert.equal(app.get('#selected-kind').textContent, '자막');
  assert.equal(app.get('#selected-open').href, subtitle.url);
  await app.get('#selected-copy').listeners.click();
  assert.equal(app.copied[0], subtitle.url);
});

test('empty general fallback can still display and select a subtitle', async () => {
  const subtitle = video('a', 'subtitle');
  const app = setup(async () => response([subtitle]));
  await app.submit();
  assert.equal(app.get('#selected-kind').textContent, '자막');
  assert.equal(app.get('#selected-open').href, subtitle.url);
});

test('switching modes ignores late results and copies only the current first result', async () => {
  const requests = [];
  const app = setup((url, options) => new Promise(resolve => { requests.push({ url, options, resolve }); }));
  app.get('#auto-copy').checked = true;
  const oldSubmit = app.submit();
  const newSubmit = app.changeMode('karaoke');
  assert.equal(requests[0].options.signal.aborted, true);
  assert.match(requests[1].url, /mode=karaoke/);
  const karaoke = video('b', 'karaoke');
  requests[1].resolve(response([karaoke]));
  await newSubmit;
  requests[0].resolve(response([video('a', 'pronunciation')]));
  await oldSubmit;
  assert.deepEqual(app.labels(), ['노래방 반주  |  채널']);
  assert.equal(app.get('#selected-open').href, karaoke.url);
  assert.deepEqual(app.copied, [karaoke.url]);
});

test('blank submission cancels the pending search and prevents late automatic copying', async () => {
  let request;
  const app = setup((url, options) => new Promise(resolve => { request = { options, resolve }; }));
  app.get('#auto-copy').checked = true;
  const pending = app.submit();
  app.get('#song-query').value = '   ';
  await app.submit();
  assert.equal(request.options.signal.aborted, true);
  request.resolve(response([video('a', 'pronunciation')]));
  await pending;
  assert.equal(app.get('#submit-btn').disabled, false);
  assert.equal(app.get('#button-text').textContent, '검색');
  assert.equal(app.get('#selected-copy').disabled, true);
  assert.equal(app.get('#status').children[0].children[0].textContent, '곡명을 입력하세요');
  assert.deepEqual(app.copied, []);
});

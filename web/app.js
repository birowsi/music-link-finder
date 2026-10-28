(() => {
  'use strict';
  const form = document.querySelector('#search-form');
  const query = document.querySelector('#song-query');
  const button = document.querySelector('#submit-btn');
  const buttonText = document.querySelector('#button-text');
  const status = document.querySelector('#status');
  const results = document.querySelector('#results');
  const count = document.querySelector('#result-count');
  const fallback = document.querySelector('#fallback');
  const fallbackUrl = document.querySelector('#fallback-url');
  const autoCopy = document.querySelector('#auto-copy');
  autoCopy.checked = false;
  let controller;

  function element(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }
  function setStatus(title, detail, kind = '') {
    status.className = `empty-state ${kind}`;
    status.replaceChildren(element('span', 'state-symbol', kind === 'error' ? '!' : kind === 'loading' ? '···' : '—'), element('h3', '', title), element('p', '', detail));
    status.hidden = false;
    results.hidden = true;
    count.hidden = true;
  }
  async function copyLink(url, button) {
    fallback.hidden = true;
    try {
      if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable');
      await navigator.clipboard.writeText(url);
      const previous = button.textContent;
      button.textContent = '복사 완료 ✓';
      window.setTimeout(() => { if (button.isConnected) button.textContent = previous; }, 1800);
    } catch {
      fallbackUrl.value = url;
      fallback.hidden = false;
      fallbackUrl.focus();
      fallbackUrl.select();
      fallback.scrollIntoView({ behavior: window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'instant' : 'smooth', block: 'nearest' });
    }
  }
  document.querySelector('#fallback-select').addEventListener('click', () => { fallbackUrl.focus(); fallbackUrl.select(); });

  function group(label, items, recommended) {
    if (!items.length) return;
    const heading = element('div', recommended ? 'group-title recommend-group' : 'group-title');
    heading.append(element('strong', '', label), element('span', '', `${items.length}개 영상`));
    results.append(heading);
    items.forEach((video, index) => {
      if (!/^https:\/\/www\.youtube\.com\/watch\?v=[a-zA-Z0-9_-]{11}$/.test(video.url)) return;
      const card = element('article', 'video-card');
      const number = element('span', 'video-index', String(index + 1).padStart(2, '0'));
      const main = element('div', 'video-main');
      const title = element('div', 'video-title', video.title);
      const channel = element('p', 'video-channel', video.channel);
      const meta = element('div', 'video-meta');
      if (recommended) meta.append(element('span', 'tag recommend', '다른 곡 추천'));
      for (const match of (Array.isArray(video.matched) ? video.matched : []).slice(0, 3)) meta.append(element('span', 'tag', match));
      const actions = element('div', 'video-actions');
      const copy = element('button', '', '링크 복사');
      copy.type = 'button';
      copy.setAttribute('aria-label', `${video.title} 링크 복사`);
      copy.addEventListener('click', () => copyLink(video.url, copy));
      const open = element('a', '', 'YouTube에서 열기 ↗');
      open.href = video.url;
      open.target = '_blank';
      open.rel = 'noopener noreferrer';
      open.setAttribute('aria-label', `${video.title} YouTube에서 새 탭으로 열기`);
      actions.append(copy, open);
      main.append(title, channel, meta, actions);
      card.append(number, main);
      results.append(card);
    });
  }
  function render(videos) {
    results.replaceChildren();
    fallback.hidden = true;
    videos = videos.filter(video => video && typeof video.url === 'string' && /^https:\/\/www\.youtube\.com\/watch\?v=[a-zA-Z0-9_-]{11}$/.test(video.url));
    if (!videos.length) { setStatus('조건에 맞는 영상이 없어요', '다른 곡명이나 검색 유형으로 다시 검색해 보세요.'); return; }
    const exact = videos.filter(item => !item.is_recommendation);
    const recs = videos.filter(item => item.is_recommendation);
    group('곡명 일치 결과', exact, false);
    group('함께 찾아볼 추천 영상', recs, true);
    status.hidden = true;
    results.hidden = false;
    count.textContent = `총 ${videos.length}개`;
    count.hidden = false;
    return { url: (exact[0] || recs[0]).url, button: results.querySelector('.video-actions button') };
  }
  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    const title = query.value.trim();
    if (!title) { query.focus(); setStatus('곡명을 입력해 주세요', '검색할 노래 제목이나 별칭을 적어 주세요.', 'error'); return; }
    controller?.abort();
    controller = new AbortController();
    const current = controller;
    button.disabled = true;
    buttonText.textContent = '검색 중…';
    fallback.hidden = true;
    setStatus('YouTube에서 영상을 찾고 있어요', '검색 결과를 비교하고 순위를 계산하고 있습니다.', 'loading');
    try {
      const mode = new FormData(form).get('mode');
      const response = await fetch(`/api/search?${new URLSearchParams({ q: title, mode })}`, { signal: current.signal });
      const data = await response.json();
      if (current.signal.aborted) return;
      if (!response.ok) throw new Error(data.error || '검색에 실패했습니다.');
      const first = render(data.results || []);
      if (autoCopy.checked && first) await copyLink(first.url, first.button);
    } catch (error) {
      if (error.name !== 'AbortError') setStatus('검색에 문제가 생겼어요', error.message || '연결 상태를 확인하고 다시 시도해 주세요.', 'error');
    } finally {
      if (!current.signal.aborted) { button.disabled = false; buttonText.textContent = '영상 찾기'; }
    }
  });
})();

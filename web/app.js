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
  const selectedTitle = document.querySelector('#selected-title');
  const selectedChannel = document.querySelector('#selected-channel');
  const selectedCopy = document.querySelector('#selected-copy');
  const selectedOpen = document.querySelector('#selected-open');
  const selectedImage = document.querySelector('#selected-image');
  const previewPlaceholder = document.querySelector('#preview-placeholder');
  const selectedKind = document.querySelector('#selected-kind');
  let selectedVideo;
  let copySequence = 0;
  autoCopy.checked = false;
  let controller;

  function element(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = displayText(text);
    return node;
  }
  function displayText(text) {
    return String(text ?? '').replace(/[\u30fb\u00b7]/g, ' ');
  }
  function selectVideo(video, card) {
    copySequence++;
    selectedCopy.textContent = '링크 복사';
    const previous = results.querySelector('.is-selected');
    previous?.classList.remove('is-selected');
    previous?.setAttribute('aria-pressed', 'false');
    selectedVideo = video;
    if (video) {
      card.classList.add('is-selected');
      card.setAttribute('aria-pressed', 'true');
      selectedImage.hidden = false; previewPlaceholder.hidden = true; selectedImage.src = thumbnailUrl(video);
      selectedKind.textContent = videoKind(video);
      selectedTitle.textContent = displayText(video.title);
      selectedChannel.textContent = displayText(video.channel);
      selectedCopy.disabled = false;
      selectedOpen.href = video.url;
      selectedOpen.hidden = false;
    } else {
      selectedTitle.textContent = '선택한 영상';
      selectedChannel.textContent = '';
      selectedKind.textContent = ''; selectedImage.hidden = true; selectedImage.removeAttribute('src'); previewPlaceholder.hidden = false;
      selectedCopy.disabled = true;
      selectedOpen.removeAttribute('href');
      selectedOpen.hidden = true;
    }
  }
  selectedImage.addEventListener('error', () => { selectedImage.hidden = true; previewPlaceholder.hidden = false; });
  selectedCopy.addEventListener('click', () => {
    if (selectedVideo) copyLink(selectedVideo.url, selectedCopy);
  });
  function setStatus(title, detail, kind = '') {
    selectVideo(null);
    document.querySelector('#results-title').textContent = '검색 결과';
    status.className = `empty-state ${kind}`;
    const message = element('div', kind === 'loading' ? 'sr-only' : '');
    message.append(element('h3', '', title), element('p', '', detail));
    status.replaceChildren(message);
    if(kind === 'loading') { const skeleton=element('div','skeleton-list'); for(let i=0;i<4;i++){const row=element('div','skeleton-row');row.append(element('span'),element('span'));skeleton.append(row);}status.append(skeleton); }
    status.hidden = false;
    results.hidden = true;
    count.hidden = true;
  }
  async function copyLink(url, button) {
    if (button.disabled) return;
    const operation = ++copySequence;
    button.disabled = true;
    fallback.hidden = true;
    try {
      if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable');
      await navigator.clipboard.writeText(url);
      if (operation !== copySequence) return;
      button.textContent = '복사 완료';
      window.setTimeout(() => {
        if (operation !== copySequence) return;
        button.textContent = '링크 복사';
        button.disabled = button === selectedCopy && !selectedVideo;
      }, 1800);
    } catch {
      if (operation !== copySequence) return;
      fallbackUrl.value = url;
      fallback.hidden = false;
      fallbackUrl.focus();
      fallbackUrl.select();
      fallback.scrollIntoView({ behavior: window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'instant' : 'smooth', block: 'nearest' });
      button.disabled = false;
    }
  }
  document.querySelector('#fallback-select').addEventListener('click', () => { fallbackUrl.focus(); fallbackUrl.select(); });

  function thumbnailUrl(video) {
    const id = video.url.match(/[?]v=([a-zA-Z0-9_-]{11})$/)?.[1];
    return id ? `https://i.ytimg.com/vi/${id}/mqdefault.jpg` : '';
  }
  function videoKind(video) {
    if (video.is_fallback) return '일반 결과';
    if (video.is_recommendation) return video.kind === 'subtitle' ? '관련 추천 · 자막' : '관련 추천';
    return { pronunciation: '한글 발음', subtitle: '자막', karaoke: '노래방 반주' }[video.kind] || '일반 결과';
  }
  function group(label, items) {
    if (!items.length) return;
    if (items[0].is_recommendation) results.append(element('h3','group-title',label));
    items.forEach(video => {
      const card=element('button','video-card'); card.type='button';card.setAttribute('aria-pressed','false');
      const image=element('img','video-thumbnail');image.alt='';image.src=thumbnailUrl(video);image.loading='lazy';image.decoding='async';
      const preview=element('span','row-preview');preview.append(image);
      image.addEventListener('error',()=>{image.hidden=true;preview.append(element('span','image-failed','미리보기 없음'));},{once:true});
      const main=element('span','video-main');
      const metadata=element('span','video-channel');metadata.textContent=`${videoKind(video)}  |  ${displayText(video.channel)}`;
      main.append(element('span','video-title',video.title),metadata);
      card.append(preview,main);card.addEventListener('click',()=>selectVideo(video,card));
      card.addEventListener('keydown',event=>{if(!['ArrowDown','ArrowUp'].includes(event.key))return;const cards=[...results.querySelectorAll('.video-card')];const next=cards[cards.indexOf(card)+(event.key==='ArrowDown'?1:-1)];if(next){event.preventDefault();next.focus();next.click();}});
      results.append(card);
    });
  }
  function render(videos) {
    results.replaceChildren();
    fallback.hidden = true;
    videos = videos.filter(video => video && typeof video.url === 'string' && /^https:\/\/www\.youtube\.com\/watch\?v=[a-zA-Z0-9_-]{11}$/.test(video.url));
    if (!videos.length) { setStatus('검색 결과가 없습니다', ''); return; }
    document.querySelector('#results-title').textContent=videos[0].is_fallback?'일반 YouTube 결과':'검색 결과';
    const exact = videos.filter(item => !item.is_recommendation && !item.is_fallback);
    const general = videos.filter(item => item.is_fallback);
    const recs = videos.filter(item => item.is_recommendation);
    group('일반 YouTube 결과', general, false);
    group('곡명 일치 결과', exact, false);
    group('함께 찾아볼 추천 영상', recs, true);
    status.hidden = true;
    results.hidden = false;
    count.textContent = `총 ${videos.length}개`;
    count.hidden = false;
    const first = general[0] || exact[0] || recs[0];
    selectVideo(first, results.querySelector('.video-card'));
    return { url: first.url, button: selectedCopy };
  }
  form.querySelectorAll('[name=mode]').forEach(input => input.addEventListener('change', () => {
    controller?.abort();
    button.disabled = false;
    buttonText.textContent = '검색';
    fallback.hidden = true;
    results.replaceChildren();
    setStatus('', '');
    if (query.value.trim()) form.requestSubmit();
  }));
  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    const title = query.value.trim();
    controller?.abort();
    controller = null;
    if (!title) {
      button.disabled = false;
      buttonText.textContent = '검색';
      query.focus(); setStatus('곡명을 입력하세요', '', 'error'); return;
    }
    controller = new AbortController();
    const current = controller;
    button.disabled = true;
    buttonText.textContent = '검색 중';
    fallback.hidden = true;
    setStatus('검색 중', '', 'loading');
    try {
      const mode = new FormData(form).get('mode');
      const response = await fetch(`/api/search?${new URLSearchParams({ q: title, mode })}`, { signal: current.signal });
      const data = await response.json();
      if (controller !== current || current.signal.aborted) return;
      if (!response.ok) throw new Error(data.error || '검색에 실패했습니다.');
      const first = render(data.results || []);
      if (autoCopy.checked && first) await copyLink(first.url, first.button);
    } catch (error) {
      if (controller === current && !current.signal.aborted && error.name !== 'AbortError') setStatus('검색 실패', error.message || '연결 상태를 확인하고 다시 시도해 주세요.', 'error');
    } finally {
      if (controller === current && !current.signal.aborted) { button.disabled = false; buttonText.textContent = '검색'; }
    }
  });
})();

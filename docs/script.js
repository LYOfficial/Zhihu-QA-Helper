const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)');

function initGallery() {
  const gallery = document.querySelector('[data-gallery]');
  const track = gallery.querySelector('.gallery-track');
  const slides = Array.from(track.querySelectorAll('.gallery-slide'));
  const caption = gallery.querySelector('.gallery-caption');
  const count = gallery.querySelector('.gallery-count');
  const thumbs = gallery.querySelector('.gallery-thumbs');
  const dialog = document.querySelector('.gallery-dialog');
  const dialogImage = dialog.querySelector('img');
  const dialogTitle = dialog.querySelector('#gallery-dialog-title');
  const original = dialog.querySelector('a');
  const arrows = [...gallery.querySelectorAll('[data-gallery-step]'), ...dialog.querySelectorAll('[data-gallery-step]')];
  let current = slides.findIndex(slide => slide.classList.contains('is-current'));
  let scrollTimer;

  const buttons = slides.map((slide, index) => {
    const button = document.createElement('button');
    const preview = document.createElement('img');
    button.className = 'gallery-thumb';
    button.type = 'button';
    button.setAttribute('aria-label', `第 ${index + 1} 张：${slide.dataset.title}`);
    button.setAttribute('aria-controls', track.id);
    preview.src = slide.href;
    preview.alt = '';
    preview.loading = 'lazy';
    button.append(preview);
    button.addEventListener('click', () => select(index));
    thumbs.append(button);
    return button;
  });

  function centerOf(slide) {
    return slide.offsetLeft + slide.offsetWidth / 2;
  }

  function centerSlide(smooth) {
    track.scrollTo({
      left: centerOf(slides[current]) - track.clientWidth / 2,
      behavior: smooth && !reducedMotion.matches ? 'smooth' : 'instant',
    });
  }

  function renderDialog() {
    const slide = slides[current];
    dialogTitle.textContent = `${current + 1} / ${slides.length} · ${slide.dataset.title}`;
    dialogImage.src = slide.href;
    dialogImage.alt = slide.querySelector('img').alt;
    original.href = slide.href;
  }

  function select(index, { scroll = true, smooth = true } = {}) {
    current = Math.max(0, Math.min(index, slides.length - 1));
    slides.forEach((slide, slideIndex) => {
      const selected = current === slideIndex;
      slide.classList.toggle('is-current', selected);
      slide.tabIndex = selected ? 0 : -1;
      buttons[slideIndex].setAttribute('aria-current', String(selected));
    });
    caption.textContent = slides[current].dataset.title;
    count.textContent = `${current + 1} / ${slides.length}`;
    arrows.forEach(arrow => {
      arrow.disabled = Number(arrow.dataset.galleryStep) < 0 ? current === 0 : current === slides.length - 1;
    });
    if (scroll) centerSlide(smooth);
    if (dialog.open) renderDialog();
  }

  arrows.forEach(arrow => arrow.addEventListener('click', () => {
    select(current + Number(arrow.dataset.galleryStep), { smooth: !dialog.open });
  }));

  function navigateWithKeyboard(event) {
    const destinations = { ArrowLeft: current - 1, ArrowRight: current + 1, Home: 0, End: slides.length - 1 };
    if (!Object.hasOwn(destinations, event.key) || event.altKey || event.ctrlKey || event.metaKey) return;
    event.preventDefault();
    const slideHadFocus = slides[current] === document.activeElement;
    select(destinations[event.key], { smooth: !dialog.open });
    if (slideHadFocus) slides[current].focus({ preventScroll: true });
  }

  gallery.addEventListener('keydown', navigateWithKeyboard);
  dialog.addEventListener('keydown', navigateWithKeyboard);
  track.addEventListener('scroll', () => {
    window.clearTimeout(scrollTimer);
    scrollTimer = window.setTimeout(() => {
      const center = track.scrollLeft + track.clientWidth / 2;
      const nearest = slides.reduce((best, slide, index) =>
        Math.abs(centerOf(slide) - center) < Math.abs(centerOf(slides[best]) - center) ? index : best, current);
      if (nearest !== current) select(nearest, { scroll: false });
    }, 150);
  }, { passive: true });

  slides.forEach((slide, index) => slide.addEventListener('click', event => {
    if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    if (index !== current) {
      event.preventDefault();
      select(index);
      slides[current].focus({ preventScroll: true });
    } else if (typeof dialog.showModal === 'function') {
      event.preventDefault();
      renderDialog();
      dialog.showModal();
    }
  }));
  dialog.querySelector('.gallery-close').addEventListener('click', () => dialog.close());
  dialog.addEventListener('click', event => { if (event.target === dialog) dialog.close(); });
  dialog.addEventListener('close', () => slides[current].focus({ preventScroll: true }));

  gallery.classList.add('is-ready');
  arrows.forEach(arrow => { arrow.hidden = false; });
  thumbs.hidden = false;
  count.hidden = false;
  select(current, { smooth: false });
  window.addEventListener('resize', () => centerSlide(false), { passive: true });
}

function initAnimations() {
  const header = document.querySelector('.site-header');
  const updateHeader = () => header.classList.toggle('is-scrolled', window.scrollY > 30);
  updateHeader();
  window.addEventListener('scroll', updateHeader, { passive: true });

  if (reducedMotion.matches || !('IntersectionObserver' in window)) return;
  const targets = document.querySelectorAll('.section-head, .feature-card, .showcase-copy, .showcase-row .screen, .steps li, .getting-started .fine-print, .download-inner > *, .footer-inner > *');
  const observer = new IntersectionObserver(entries => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      entry.target.classList.remove('is-pending');
      observer.unobserve(entry.target);
    }
  }, { rootMargin: '0px 0px -32px', threshold: 0.1 });

  const counts = new Map();
  targets.forEach(target => {
    const parent = target.parentElement;
    const index = counts.get(parent) ?? 0;
    counts.set(parent, index + 1);
    target.style.setProperty('--reveal-delay', `${Math.min(index * 65, 240)}ms`);
    target.classList.add('reveal', 'is-pending');
    observer.observe(target);
  });
}

function initDownloads() {
  const repository = 'LYOfficial/Zhihu-QA-Helper';
  const releasePage = `https://github.com/${repository}/releases/latest`;
  const endpoint = `https://api.github.com/repos/${repository}/releases/latest`;
  const mirrorPrefix = 'https://gh-proxy.com/';
  const download = document.querySelector('#release-download');
  const status = document.querySelector('#release-status');
  const mirror = document.querySelector('#release-mirror');
  const options = document.querySelector('#release-options');
  const hint = document.querySelector('#mirror-hint');
  const notes = document.querySelector('#release-notes');
  const checksum = document.querySelector('#release-checksum');
  let installerUrl;
  let lastChecked = 0;

  function updateLink() {
    if (!installerUrl) return;
    download.href = mirror.checked ? `${mirrorPrefix}${installerUrl}` : installerUrl;
    download.textContent = mirror.checked ? '镜像下载 Windows 安装包' : '下载 Windows 安装包';
    hint.hidden = !mirror.checked;
  }

  function assetUrl(release, name) {
    const expected = `https://github.com/${repository}/releases/download/${encodeURIComponent(release.tag_name)}/${encodeURIComponent(name)}`;
    return release.assets.some(asset => asset.name === name && asset.state === 'uploaded' && asset.browser_download_url === expected) ? expected : null;
  }

  async function loadRelease() {
    lastChecked = Date.now();
    const controller = new AbortController();
    const timeout = window.setTimeout(() => controller.abort(), 6500);
    try {
      const response = await fetch(endpoint, { signal: controller.signal, cache: 'no-cache', referrerPolicy: 'no-referrer' });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const release = await response.json();
      if (release.draft || release.prerelease || !/^v\d+\.\d+\.\d+$/.test(release.tag_name) || !Array.isArray(release.assets)) {
        throw new Error('没有可下载的正式版本');
      }
      const name = `Zhihu-QA-Helper_${release.tag_name.slice(1)}_x64-setup.exe`;
      const versionPage = `https://github.com/${repository}/releases/tag/${release.tag_name}`;
      installerUrl = assetUrl(release, name);
      status.hidden = false;
      if (!installerUrl) {
        mirror.checked = false;
        options.hidden = true;
        download.href = versionPage;
        download.textContent = '查看最新 Release';
        status.textContent = `${release.tag_name} 暂无 Windows x64 安装包。`;
        return;
      }
      const asset = release.assets.find(item => item.name === name);
      const size = asset.size > 0 ? ` · ${(asset.size / 1048576).toFixed(1)} MiB` : '';
      status.textContent = `最新正式版 ${release.tag_name} · Windows x64${size}`;
      notes.href = versionPage;
      const hashUrl = assetUrl(release, 'SHA256SUMS.txt');
      checksum.hidden = !hashUrl;
      if (hashUrl) checksum.href = hashUrl;
      options.hidden = false;
      updateLink();
    } catch {
      if (!installerUrl) {
        download.href = releasePage;
        status.hidden = false;
        status.textContent = '暂时无法获取版本信息，可到 GitHub 查看最新发布。';
      }
    } finally {
      window.clearTimeout(timeout);
    }
  }

  mirror.checked = false;
  mirror.addEventListener('change', updateLink);
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden && Date.now() - lastChecked > 60000) void loadRelease();
  });
  void loadRelease();
}

initGallery();
initAnimations();
initDownloads();

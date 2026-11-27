/* MONO.STT 官网 - 交互脚本（沿用站点原有组件行为） */
(function () {
  'use strict';

  var REPO = 'https://github.com/Rui333333/MONO.STT';
  var RELEASES = REPO + '/releases';
  var DL = RELEASES + '/download/';

  /* ---------- 移动端菜单 ---------- */
  var menuBtn = document.querySelector('.menu-btn');
  var mobileNav = document.querySelector('.mobile-nav');
  if (menuBtn && mobileNav) {
    menuBtn.addEventListener('click', function () {
      var open = mobileNav.classList.toggle('open');
      menuBtn.setAttribute('aria-label', open ? '关闭菜单' : '打开菜单');
    });
    mobileNav.querySelectorAll('a').forEach(function (a) {
      a.addEventListener('click', function () {
        mobileNav.classList.remove('open');
        menuBtn.setAttribute('aria-label', '打开菜单');
      });
    });
  }

  /* ---------- FAQ 手风琴 ---------- */
  document.querySelectorAll('.faq').forEach(function (faq) {
    faq.querySelectorAll('.faq-item').forEach(function (item) {
      var btn = item.querySelector('button');
      var body = item.querySelector('.body');
      var sign = btn.querySelector('span');
      btn.addEventListener('click', function () {
        var willOpen = btn.getAttribute('aria-expanded') !== 'true';
        faq.querySelectorAll('.faq-item').forEach(function (other) {
          var ob = other.querySelector('button');
          var obody = other.querySelector('.body');
          ob.setAttribute('aria-expanded', 'false');
          ob.querySelector('span').textContent = '+';
          if (obody) obody.hidden = true;
        });
        if (willOpen) {
          btn.setAttribute('aria-expanded', 'true');
          sign.textContent = '−';
          if (body) body.hidden = false;
        }
      });
    });
  });

  /* ---------- 首页：转录工作流切换 ---------- */
  var workflows = {
    import: {
      title: '导入转录',
      prompt: '访谈录音.m4a · Whisper Large v3 Turbo',
      result: '00:00:03 今天我们聊聊产品从零到一的过程……'
    },
    review: {
      title: '时间轴校对',
      prompt: '点击 00:01:12 跳转回听并修改这一句',
      result: '00:01:12 「先把素材整理成逐字稿」→ 已校对并保存'
    },
    export: {
      title: '批量导出',
      prompt: '勾选 12 条记录，导出 TXT 与 SRT',
      result: 'MONO.STT_导出_20261127.zip · 12 个文件 · TXT + SRT'
    }
  };
  var pills = document.querySelectorAll('.pills .pill');
  if (pills.length) {
    var aTitle = document.querySelector('[data-agent-title]');
    var aPrompt = document.querySelector('[data-agent-prompt]');
    var aResult = document.querySelector('[data-agent-result]');
    pills.forEach(function (pill) {
      pill.addEventListener('click', function () {
        var data = workflows[pill.dataset.agent];
        if (!data) return;
        pills.forEach(function (p) { p.classList.toggle('active', p === pill); });
        aTitle.textContent = data.title;
        aPrompt.textContent = data.prompt;
        aResult.textContent = data.result;
      });
    });
  }

  /* ---------- 首页：下载卡片切换（版本 / 获取方式） ---------- */
  var downloads = {
    mac: {
      latest: {
        meta: 'v0.1.7 · DMG · 108.2 MB',
        label: '下载 DMG 安装包',
        url: DL + 'v0.1.7/MONO.STT_0.1.7_arm64.dmg'
      },
      prev: {
        meta: 'v0.1.6 · DMG · 106.7 MB',
        label: '下载上一版 DMG',
        url: DL + 'v0.1.6/MONO.STT_0.1.6_arm64.dmg'
      }
    },
    windows: {
      latest: {
        meta: 'v0.1.7 · EXE 安装版 · 139.5 MB',
        label: '下载 EXE 安装版',
        url: DL + 'v0.1.7/MONO.STT_0.1.7_x64-setup.exe'
      },
      prev: {
        meta: 'v0.1.6 · EXE 安装版 · 139.6 MB',
        label: '下载上一版 EXE',
        url: DL + 'v0.1.6/MONO.STT_0.1.6_x64-setup.exe'
      }
    },
    source: {
      readme: {
        meta: 'Rust + Python · Windows / macOS',
        label: '查看构建说明',
        url: REPO + '#从源码构建'
      },
      release: {
        meta: '全部版本 · EXE / DMG / 签名',
        label: '浏览 GitHub Releases',
        url: RELEASES
      }
    }
  };
  document.querySelectorAll('.dl-card').forEach(function (card) {
    var platform = card.dataset.card;
    var buttons = card.querySelectorAll('.os-tabs button');
    var meta = card.querySelector('[data-dl-meta]');
    var dlBtn = card.querySelector('[data-dl-btn]');
    buttons.forEach(function (btn) {
      btn.addEventListener('click', function () {
        buttons.forEach(function (b) { b.classList.toggle('active', b === btn); });
        var data = downloads[platform] && downloads[platform][btn.dataset.os];
        if (!data || !meta || !dlBtn) return;
        meta.textContent = data.meta;
        dlBtn.textContent = data.label;
        dlBtn.href = data.url;
      });
    });
  });

  /* ---------- 首页：标记当前系统 ---------- */
  var ua = navigator.userAgent;
  var current = /Windows/i.test(ua) ? 'windows'
    : (/Macintosh|Mac OS X/i.test(ua)) ? 'mac' : 'source';
  document.querySelectorAll('.dl-card').forEach(function (card) {
    var badge = card.querySelector(':scope > p.current-badge');
    if (badge) badge.remove();
    card.classList.remove('current');
    if (card.dataset.card === current) {
      card.classList.add('current');
      var p = document.createElement('p');
      p.className = 'current-badge';
      p.textContent = '当前系统';
      card.insertBefore(p, card.firstChild);
    }
  });

  /* ---------- 首页：快捷键 D / M ---------- */
  document.addEventListener('keydown', function (e) {
    var tag = e.target && e.target.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA') return;
    var key = e.key.toLowerCase();
    if (key === 'd') {
      var d = document.getElementById('download');
      if (d) d.scrollIntoView({ behavior: 'smooth' });
    } else if (key === 'm') {
      var f = document.getElementById('features');
      if (f) f.scrollIntoView({ behavior: 'smooth' });
    }
  });

  /* ---------- 开源页：获取方式切换（两种都免费） ---------- */
  var billBtns = document.querySelectorAll('.bill-toggle button');
  if (billBtns.length) {
    var period = document.querySelector('[data-price-period]');
    var blurb = document.querySelector('[data-price-blurb]');
    var cta = document.querySelector('[data-price-cta]');
    var options = {
      year: {
        period: 'GitHub Releases · 预编译安装包',
        blurb: '官方安装包内置转录引擎，经过签名与冒烟验证；下载即可在 Windows 与 macOS 上使用。',
        cta: '下载安装包',
        href: 'index.html#download'
      },
      life: {
        period: 'MIT License · 从源码构建',
        blurb: '从同一份代码生成 Windows EXE 与 macOS DMG，适合想自己控制构建、签名和模型目录的人。',
        cta: '查看构建说明',
        href: REPO + '#从源码构建'
      }
    };
    billBtns.forEach(function (btn) {
      btn.addEventListener('click', function () {
        var opt = options[btn.dataset.bill];
        if (!opt) return;
        billBtns.forEach(function (b) {
          b.classList.toggle('active', b === btn);
          b.setAttribute('aria-selected', b === btn ? 'true' : 'false');
        });
        if (period) period.textContent = opt.period;
        if (blurb) blurb.textContent = opt.blurb;
        if (cta) {
          cta.textContent = opt.cta;
          cta.setAttribute('href', opt.href);
        }
      });
    });
  }
})();

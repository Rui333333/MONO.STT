function animateReadingScroll(viewport, row, motion) {
  const bounds = viewport.getBoundingClientRect();
  const rowBounds = row.getBoundingClientRect();
  const center = viewport.scrollTop + rowBounds.top - bounds.top - viewport.clientTop +
    (rowBounds.height - viewport.clientHeight) / 2;
  motion.target = Math.max(0, Math.min(center, viewport.scrollHeight - viewport.clientHeight));

  if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
    stopReadingScroll(motion);
    viewport.scrollTop = motion.target;
    return;
  }
  if (motion.frame !== null) return;

  // Preserve fractional positions and velocity when the active row changes mid-scroll.
  motion.position = viewport.scrollTop;
  motion.lastTime = performance.now();
  const rate = 12;
  function tick(now) {
    const elapsed = Math.max(0, Math.min((now - motion.lastTime) / 1000, 0.05));
    motion.lastTime = now;
    const distance = motion.position - motion.target;
    const slope = motion.velocity + rate * distance;
    const decay = Math.exp(-rate * elapsed);
    const remaining = (distance + slope * elapsed) * decay;
    motion.velocity = (motion.velocity - rate * slope * elapsed) * decay;
    motion.position = motion.target + remaining;
    if (Math.abs(remaining) < 0.5 && Math.abs(motion.velocity) < 5) {
      viewport.scrollTop = motion.target;
      stopReadingScroll(motion);
      return;
    }
    viewport.scrollTop = motion.position;
    motion.frame = requestAnimationFrame(tick);
  }
  motion.frame = requestAnimationFrame(tick);
}

function stopReadingScroll(motion) {
  if (motion.frame !== null) cancelAnimationFrame(motion.frame);
  motion.frame = null;
  motion.velocity = 0;
}

function useReadingPosition(React, recordId, segmentId, timeline, playing, follow, query, clearSearch) {
  const viewport = React.useRef(null);
  const revealAfterFilter = React.useRef(false);
  const followedRow = React.useRef(null);
  const motion = React.useRef({frame:null, velocity:0});
  const [outside, setOutside] = React.useState(false);
  const activeRow = () => viewport.current?.querySelector('[data-reading-row]');
  const centerRow = row => animateReadingScroll(viewport.current, row, motion.current);
  React.useLayoutEffect(() => {
    const panel = viewport.current;
    const cancel = () => stopReadingScroll(motion.current);
    panel?.addEventListener('wheel', cancel, {passive:true});
    panel?.addEventListener('touchstart', cancel, {passive:true});
    return () => {
      cancel();
      panel?.removeEventListener('wheel', cancel);
      panel?.removeEventListener('touchstart', cancel);
    };
  }, [recordId,timeline,playing,follow]);
  React.useLayoutEffect(() => {
    setOutside(false);
    if (!playing || !timeline || !segmentId || !viewport.current) {
      revealAfterFilter.current = false;
      followedRow.current = null;
      return;
    }
    const row = activeRow();
    if (!row) {
      followedRow.current = null;
      setOutside(Boolean(query));
      return;
    }
    const reveal = revealAfterFilter.current;
    revealAfterFilter.current = false;
    if (reveal || (follow && followedRow.current !== row)) {
      centerRow(row);
    }
    followedRow.current = follow ? row : null;
    const observer = new IntersectionObserver(([entry]) => {
      setOutside(!entry.isIntersecting);
    }, {root:viewport.current});
    observer.observe(row);
    return () => observer.disconnect();
  }, [recordId,segmentId,timeline,playing,follow,query]);
  function reveal() {
    const row = activeRow();
    if (row) centerRow(row);
    else if(query) {
      revealAfterFilter.current = true;
      clearSearch();
    }
  }
  return {viewport,visible:outside && playing && timeline && Boolean(segmentId),reveal};
}

function indexReadingSegments(segments) {
  const endTimes = [];
  let end = -Infinity;
  for (let index = 0; index < segments.length; index++) {
    const segment = segments[index];
    if (!Number.isFinite(segment.startSec) || !Number.isFinite(segment.endSec) ||
        (index > 0 && segment.startSec < segments[index - 1].startSec)) return null;
    end = Math.max(end, segment.endSec);
    endTimes.push(end);
  }
  return endTimes;
}

function readingSegmentAt(segments, endTimes, time) {
  if (!endTimes) return segments.find(segment => time >= segment.startSec && time < segment.endSec)?.id ?? null;
  let first = 0;
  let last = endTimes.length;
  while (first < last) {
    const middle = (first + last) >>> 1;
    if (endTimes[middle] > time) last = middle;
    else first = middle + 1;
  }
  const segment = segments[first];
  return segment && segment.startSec <= time ? segment.id : null;
}

function useReadingAudio(React, record, notify) {
  const [availability, setAvailability] = React.useState(null);
  const current = React.useRef(null);
  const audioKey = record.audioPath ? record.id + ':' + record.audioPath : null;
  React.useLayoutEffect(() => {
    const session = {audioKey, request:0};
    current.current = session;
    return () => { current.current = null; };
  }, [audioKey]);

  async function check(failed) {
    const session = current.current;
    if (!audioKey || session?.audioKey !== audioKey) return;
    if (!failed && session.feedback) return;
    session.feedback = failed;
    const request = ++session.request;
    let status;
    try { status = await desktop.invoke('store_audio_status', {recordId:record.id}); }
    catch { status = 'unavailable'; }
    if (current.current !== session || request !== session.request) return;
    session.feedback = false;
    setAvailability({audioKey, status});
    if (failed) notify(status === 'missing' ? '找不到音频文件，可能已被移动或删除。'
      : '无法播放音频，请检查文件是否可读取。');
  }

  React.useEffect(() => { void check(false); }, [audioKey]);
  return {
    missing:availability?.audioKey === audioKey && availability?.status === 'missing',
    probe:() => { void check(false); },
    failed:error => { if (error?.name !== 'AbortError') void check(true); },
    playing:() => {
      if (audioKey && current.current?.audioKey === audioKey) {
        current.current.request++;
        current.current.feedback = false;
        setAvailability(null);
      }
    },
  };
}

function createReadingTimeline(React, jsx, formatTime, DeleteIcon) {
  const Row = React.memo(function TranscriptRow({segment, active, canRemove, readOnly, canSeek, actions}) {
    const lines = Math.max(1, Math.ceil(segment.text.length / 44));
    return jsx('div', {
      'data-reading-row':active ? '' : undefined,
      className:'reading-row group flex items-start gap-4 -mx-3 px-3 rounded-md',
      style:{'--reading-row-estimate':(lines * 22.5 + 10 + (segment.speaker ? 17 : 0)) + 'px'},
      children:[
        jsx('button', {
          type:'button', disabled:!canSeek, onClick:() => actions.current.seek(segment.startSec),
          className:'reading-timestamp font-mono text-[11.5px] font-medium tabular-nums pt-1 w-11 shrink-0 text-left',
          children:formatTime(segment.startSec),
        }),
        jsx('div', {className:'flex-1 min-w-0', children:[
          segment.speaker && jsx('div', {
            className:'text-[11.5px] font-semibold text-neutral-900 dark:text-neutral-100 mb-0.5',
            children:segment.speaker,
          }),
          jsx('textarea', {
            value:segment.text, readOnly, onChange:event => actions.current.edit(segment.id, event.target.value),
            rows:lines, placeholder:'输入校对内容...',
            className:'reading-segment w-full text-[15px] text-neutral-800 dark:text-neutral-200 bg-transparent resize-none border-none p-0 focus:outline-none focus:ring-0',
          }),
        ]}),
        canRemove && jsx('button', {
          type:'button', onClick:() => actions.current.remove(segment.id), 'aria-label':'删除段落',
          className:'reading-row-delete p-1 text-neutral-500 transition-opacity shrink-0',
          children:jsx(DeleteIcon,{className:'w-3.5 h-3.5'}),
        }),
      ],
    });
  });

  const Group = React.memo(function TranscriptGroup({segments, activeId, canRemove, readOnly, canSeek, actions, viewport}) {
    const element = React.useRef(null);
    const measuredHeight = React.useRef(0);
    const [nearby, setNearby] = React.useState(false);
    const [focused, setFocused] = React.useState(false);
    const estimatedHeight = React.useMemo(() => segments.reduce((height, segment) =>
      height + Math.max(1, Math.ceil(segment.text.length / 44)) * 22.5 +
        (segment.speaker ? 17 : 0) + 12, -2), [segments]);
    const mounted = nearby || focused || activeId !== null;

    React.useEffect(() => {
      const node = element.current;
      if (!node || !viewport.current) return;
      const observer = new IntersectionObserver(([entry]) => {
        if (!node.getClientRects().length) return;
        if (!entry.isIntersecting && node.firstElementChild) {
          measuredHeight.current = node.getBoundingClientRect().height;
        }
        setNearby(entry.isIntersecting);
      }, {root:viewport.current, rootMargin:'520px 0px'});
      observer.observe(node);
      return () => observer.disconnect();
    }, [viewport]);

    React.useLayoutEffect(() => {
      const node = element.current;
      if (!mounted || !node) return;
      const observer = new ResizeObserver(([entry]) => {
        if (entry.contentRect.height > 0) measuredHeight.current = entry.contentRect.height;
      });
      observer.observe(node);
      return () => observer.disconnect();
    }, [mounted]);

    return jsx('div', {
      ref:element, className:'reading-rows',
      style:mounted ? undefined : {height:measuredHeight.current || estimatedHeight},
      onFocus:() => setFocused(true),
      onBlur:event => {
        if (!event.currentTarget.contains(event.relatedTarget)) setFocused(false);
      },
      children:mounted ? segments.map(segment => jsx(Row, {
        segment, active:segment.id === activeId, canRemove, readOnly, canSeek, actions,
      }, segment.id)) : null,
    });
  });

  return React.memo(function ReadingTimeline({segments, activeId, query, canRemove, readOnly, canSeek, actions, viewport}) {
    const [groups, groupIds] = React.useMemo(() => {
      const groups = [];
      const groupIds = new Map();
      const groupSize = 32;
      for (let index = 0; index < segments.length; index += groupSize) {
        const group = segments.slice(index, index + groupSize);
        for (const segment of group) groupIds.set(segment.id, groups.length);
        groups.push(group);
      }
      return [groups, groupIds];
    }, [segments]);
    const activeGroup = groupIds.get(activeId);
    return jsx('div', {className:'reading-rows', children:groups.length ?
      groups.map((group,index) => jsx(Group, {
        segments:group, activeId:index === activeGroup ? activeId : null, canRemove, readOnly, canSeek, actions, viewport,
      }, group[0].id)) :
      jsx('p',{className:'py-10 text-center text-[13px] text-neutral-400',children:`未在正文中找到「${query}」`}),
    });
  });
}

function createReadingPreview(React, jsx, markdown, plaintext) {
  return React.memo(function ReadingPreview({record, options, format}) {
    const lines = (format === 'markdown' ? markdown : plaintext)(record, options).split('\n');
    const chunks = [];
    const chunkSize = 48;
    for (let index = 0; index < lines.length; index += chunkSize) {
      const chunk = lines.slice(index, index + chunkSize);
      const font = format === 'markdown' ? 'font-mono text-[13px]' : 'font-sans text-[15px]';
      chunks.push(jsx('pre', {
        className:'reading-preview-chunk reading-copy ' + font +
          ' text-neutral-800 dark:text-neutral-200 whitespace-pre-wrap break-words select-text',
        style:{'--reading-preview-estimate':(chunk.length * (format === 'markdown' ? 19.5 : 22.5)) + 'px'},
        children:chunk.join('\n'),
      }, index));
    }
    return chunks;
  }, (previous, next) => !next.visible ||
    (previous.record === next.record && previous.options === next.options && previous.format === next.format));
}

function createReadingMenu(React, jsx, icons) {
  const formats = [['txt', '纯文本 (.txt)'], ['markdown', 'Markdown (.md)'], ['srt', '字幕 (.srt)']];
  const options = [
    ['includeMetadata', '包含元数据头'],
    ['includeTimestamps', '保留时间戳'],
    ['includeSpeakers', '保留说话人'],
  ];

  return function ReadingMenu({starred, onStar, copied, onCopy, feishu, exportOptions, onOptions, onExport}) {
    const [open, setOpen] = React.useState(false);
    const menu = React.useRef(null);
    const trigger = React.useRef(null);
    const panelId = React.useId();

    React.useEffect(() => {
      if (!open) return;
      const dismissOutside = event => {
        if (!menu.current?.contains(event.target)) setOpen(false);
      };
      const escape = event => {
        if (event.key === 'Escape' && !document.querySelector('dialog[open]')) {
          setOpen(false);
          trigger.current?.focus();
        }
      };
      document.addEventListener('pointerdown', dismissOutside);
      document.addEventListener('keydown', escape);
      return () => {
        document.removeEventListener('pointerdown', dismissOutside);
        document.removeEventListener('keydown', escape);
      };
    }, [open]);

    function iconButton(label, Icon, props) {
      return jsx('button', {
        type:'button', className:'reading-export-format reading-menu-action',
        ...props,
        children:[
          jsx(Icon, {className:'reading-tool-icon', 'aria-hidden':true,
            strokeWidth:1.65,
            fill:Icon === icons.star && starred ? 'currentColor' : 'none'}),
          jsx('span',{children:label}),
        ],
      });
    }

    return jsx('div', {ref:menu,className:'reading-export',children:[
      jsx('button', {
        type:'button',className:'reading-more-trigger',ref:trigger, onClick:()=>setOpen(value=>!value),
        'aria-expanded':open,'aria-controls':panelId,children:'更多操作',
      }),
      jsx('span',{className:'reading-tool-status',role:'status',children:copied ? '已复制到剪贴板' : ''}),
      open && jsx('div',{id:panelId,className:'reading-export-panel',children:[
          jsx('div',{className:'reading-menu-common',children:[
            iconButton(starred ? '取消收藏' : '收藏记录', icons.star, {onClick:onStar, 'aria-pressed':starred}),
            iconButton(copied ? '已复制' : '复制', copied ? icons.check : icons.copy, {onClick:onCopy}),
            feishu,
          ]}),
          ...formats.map(([format,label])=>jsx('button',{
            type:'button',className:'reading-export-format',onClick:()=>{
              setOpen(false);
              trigger.current?.focus();
              onExport(format);
            },children:'导出'+label,
          },format)),
          jsx('fieldset',{className:'reading-export-options',children:[
            jsx('legend',{children:'导出选项'}),
            ...options.map(([name,label])=>jsx('label',{children:[
              jsx('span',{children:label}),
              jsx('input',{type:'checkbox',checked:exportOptions[name],
                onChange:event=>onOptions({...exportOptions,[name]:event.target.checked})}),
            ]},name)),
          ]}),
        ]}),
    ]});
  };
}

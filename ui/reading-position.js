function useReadingPosition(React, recordId, segmentId, timeline, playing, query, clearSearch) {
  const viewport = React.useRef(null);
  const revealAfterFilter = React.useRef(false);
  const [outside, setOutside] = React.useState(false);
  const activeRow = () => viewport.current?.querySelector('[data-reading-row]');
  React.useLayoutEffect(() => {
    setOutside(false);
    if (!playing || !timeline || !segmentId || !viewport.current) {
      revealAfterFilter.current = false;
      return;
    }
    const row = activeRow();
    if (!row) {
      setOutside(Boolean(query));
      return;
    }
    if (revealAfterFilter.current) {
      revealAfterFilter.current = false;
      row.scrollIntoView({block:'center',behavior:'instant'});
    }
    const observer = new IntersectionObserver(([entry]) => {
      setOutside(!entry.isIntersecting);
    }, {root:viewport.current});
    observer.observe(row);
    return () => observer.disconnect();
  }, [recordId,segmentId,timeline,playing,query]);
  function reveal() {
    const row = activeRow();
    if (row) row.scrollIntoView({block:'center',behavior:'instant'});
    else if(query) {
      revealAfterFilter.current = true;
      clearSearch();
    }
  }
  return {viewport,visible:outside && playing && timeline && Boolean(segmentId),reveal};
}

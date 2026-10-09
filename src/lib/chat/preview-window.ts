export interface PreviewRect { x: number; y: number; width: number; height: number }
export interface PreviewViewport { width: number; height: number }
export const PREVIEW_HEADER_HEIGHT = 42;
const MARGIN = 8;

/** Keep every window control reachable, including after display/viewport changes. */
export function boundPreview(rect: PreviewRect, viewport: PreviewViewport, minimized = false): PreviewRect {
  const availableWidth = Math.max(1, viewport.width - 2 * MARGIN);
  const availableHeight = Math.max(1, viewport.height - 2 * MARGIN);
  const width = Math.min(availableWidth, Math.max(280, rect.width));
  const height = Math.min(availableHeight, Math.max(220, rect.height));
  const visibleHeight = minimized ? Math.min(PREVIEW_HEADER_HEIGHT, availableHeight) : height;
  return {
    width, height,
    x: Math.max(MARGIN, Math.min(rect.x, viewport.width - width - MARGIN)),
    y: Math.max(MARGIN, Math.min(rect.y, viewport.height - visibleHeight - MARGIN)),
  };
}

export function centerPreview(viewport: PreviewViewport): PreviewRect {
  const width = Math.min(960, viewport.width * 0.8);
  const height = Math.min(760, viewport.height * 0.8);
  return boundPreview({x:(viewport.width-width)/2,y:(viewport.height-height)/2,width,height},viewport);
}

export function resizePreview(rect: PreviewRect, dx: number, dy: number, viewport: PreviewViewport): PreviewRect {
  return boundPreview({...rect,
    width: Math.min(viewport.width - rect.x - MARGIN, Math.max(280, rect.width + dx)),
    height: Math.min(viewport.height - rect.y - MARGIN, Math.max(220, rect.height + dy)),
  },viewport);
}

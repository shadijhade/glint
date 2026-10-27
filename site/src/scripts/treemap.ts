/*
  A squarified treemap (Bruls, Huizing and van Wijk, 2000): tiles kept as close to square as the values
  allow, biggest first. The Size chapter draws one at build time and the WebGL field fills another with
  points, so both come from here.
*/

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Tile<T> extends Rect {
  item: T;
}

export function squarify<T>(items: readonly T[], value: (item: T) => number, box: Rect): Tile<T>[] {
  const total = items.reduce((sum, item) => sum + value(item), 0);
  if (!total || box.w <= 0 || box.h <= 0) return [];
  const scale = (box.w * box.h) / total;
  const nodes = items.map((item) => ({ item, area: value(item) * scale })).sort((a, b) => b.area - a.area);
  const out: Tile<T>[] = [];
  let { x, y, w, h } = box;

  // The worst aspect ratio in a row laid along a side of this length.
  const worst = (row: typeof nodes, side: number) => {
    let sum = 0;
    let max = 0;
    let min = Infinity;
    for (const n of row) {
      sum += n.area;
      max = Math.max(max, n.area);
      min = Math.min(min, n.area);
    }
    return Math.max((side * side * max) / (sum * sum), (sum * sum) / (side * side * min));
  };

  const place = (row: typeof nodes) => {
    const sum = row.reduce((s, n) => s + n.area, 0);
    if (w >= h) {
      const across = sum / h;
      let at = y;
      for (const n of row) {
        const along = n.area / across;
        out.push({ item: n.item, x, y: at, w: across, h: along });
        at += along;
      }
      x += across;
      w -= across;
    } else {
      const across = sum / w;
      let at = x;
      for (const n of row) {
        const along = n.area / across;
        out.push({ item: n.item, x: at, y, w: along, h: across });
        at += along;
      }
      y += across;
      h -= across;
    }
  };

  let row: typeof nodes = [];
  for (const node of nodes) {
    const side = Math.min(w, h);
    if (!row.length || worst([...row, node], side) <= worst(row, side)) row.push(node);
    else {
      place(row);
      row = [node];
    }
  }
  if (row.length) place(row);
  return out;
}

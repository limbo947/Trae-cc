/**
 * 用量条的统一口径：返回「剩余占比」（0–100）。
 *
 * 为什么抽成共享函数而不是各自内联：账号卡片与列表行都要画同一条表示剩余的绿色条，
 * 两边各写一遍迟早会出现「一边按已用、一边按剩余」的口径漂移。
 *
 * 为什么要夹取：真实数据会越界——超支时剩余为负、赠送额度会让剩余超过总额度，
 * 直接拿去当 width 会得到负宽度或溢出的填充。
 */
export function leftPercentOf(left: number, total: number): number {
  if (!(total > 0)) return 0;
  return Math.min(Math.max((left / total) * 100, 0), 100);
}

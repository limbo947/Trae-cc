export function About() {
  return (
    <div className="about-page">
      <div className="about-card">
        {/* 头部横排 */}
        <div className="about-header">
          <img src="./logo.png" alt="Trae账号管理" className="about-logo" />
          <div className="about-header-text">
            <div className="title-row">
              <h1 className="about-title">Trae账号管理</h1>
              <span className="version">v1.0.5</span>
            </div>
          </div>
        </div>

        {/* 说明和信息横向排列 */}
        <div className="about-intro-section">
          <p className="about-desc">
            本地管理多个 Trae CN 账号的桌面工具：账号本地存储、一键切换 IDE 登录态与机器码、用量查询与统计图表。
            基于
            <a
              href="https://github.com/S-Trespassing/Trae账号管理"
              target="_blank"
              rel="noopener noreferrer"
              className="original-link"
            >
              原作者项目
            </a>
            进行二次开发。
          </p>

          <div className="about-info">
            <a
              href="https://github.com/HHH9201/Trae-CC.git"
              target="_blank"
              rel="noopener noreferrer"
              className="github-link"
            >
              <span className="label">GitHub</span>
              <span className="value">HHH9201/Trae-CC</span>
            </a>
          </div>
        </div>

        {/* 页脚 */}
        <div className="about-footer">
          Made with ❤️ by HJH · MIT License
        </div>
      </div>
    </div>
  );
}
const {
  Document,
  Packer,
  Paragraph,
  TextRun,
  Table,
  TableRow,
  TableCell,
  Header,
  Footer,
  AlignmentType,
  LevelFormat,
  HeadingLevel,
  BorderStyle,
  WidthType,
  ShadingType,
  VerticalAlign,
  PageNumber,
  convertInchesToTwip,
} = require("docx");
const fs = require("fs");
const path = require("path");

// A4 content width: 11906 - 1134*2 ≈ 9638 DXA (0.79" margins)
const PAGE_W = 11906;
const PAGE_H = 16838;
const MARGIN = 1008; // 0.7"
const CONTENT_W = PAGE_W - MARGIN * 2; // 9900

const teal = "0F766E";
const tealDark = "115E59";
const tealSoft = "CCFBF1";
const grayBorder = "D1D5DB";
const grayHeader = "F3F4F6";
const amberSoft = "FEF3C7";
const textMuted = "4B5563";
const black = "111827";

const thinBorder = { style: BorderStyle.SINGLE, size: 4, color: grayBorder };
const borders = { top: thinBorder, bottom: thinBorder, left: thinBorder, right: thinBorder };
const noBorder = {
  top: { style: BorderStyle.NONE, size: 0, color: "FFFFFF" },
  bottom: { style: BorderStyle.NONE, size: 0, color: "FFFFFF" },
  left: { style: BorderStyle.NONE, size: 0, color: "FFFFFF" },
  right: { style: BorderStyle.NONE, size: 0, color: "FFFFFF" },
};

function p(text, opts = {}) {
  const {
    bold = false,
    size = 22,
    color = black,
    italic = false,
    align,
    spacingAfter = 120,
    spacingBefore = 0,
    font = "Arial",
  } = opts;
  return new Paragraph({
    alignment: align,
    spacing: { after: spacingAfter, before: spacingBefore, line: 276 },
    children: [
      new TextRun({
        text,
        bold,
        italics: italic,
        size,
        color,
        font,
      }),
    ],
  });
}

function richP(runs, opts = {}) {
  const { align, spacingAfter = 120, spacingBefore = 0 } = opts;
  return new Paragraph({
    alignment: align,
    spacing: { after: spacingAfter, before: spacingBefore, line: 276 },
    children: runs.map((r) =>
      new TextRun({
        text: r.text,
        bold: r.bold || false,
        italics: r.italic || false,
        size: r.size || 22,
        color: r.color || black,
        font: "Arial",
      })
    ),
  });
}

function h1(text) {
  return new Paragraph({
    heading: HeadingLevel.HEADING_1,
    spacing: { before: 320, after: 160 },
    border: {
      bottom: { style: BorderStyle.SINGLE, size: 12, color: teal, space: 4 },
    },
    children: [new TextRun({ text, bold: true, size: 28, color: tealDark, font: "Arial" })],
  });
}

function h2(text) {
  return new Paragraph({
    heading: HeadingLevel.HEADING_2,
    spacing: { before: 240, after: 120 },
    children: [new TextRun({ text, bold: true, size: 24, color: teal, font: "Arial" })],
  });
}

function bullet(text, ref = "bullets") {
  return new Paragraph({
    numbering: { reference: ref, level: 0 },
    spacing: { after: 80, line: 276 },
    children: [new TextRun({ text, size: 22, font: "Arial", color: black })],
  });
}

function bulletRich(runs, ref = "bullets") {
  return new Paragraph({
    numbering: { reference: ref, level: 0 },
    spacing: { after: 80, line: 276 },
    children: runs.map(
      (r) =>
        new TextRun({
          text: r.text,
          bold: r.bold || false,
          size: r.size || 22,
          font: "Arial",
          color: r.color || black,
        })
    ),
  });
}

function numbered(text, ref) {
  return new Paragraph({
    numbering: { reference: ref, level: 0 },
    spacing: { after: 80, line: 276 },
    children: [new TextRun({ text, size: 22, font: "Arial", color: black })],
  });
}

function numberedRich(runs, ref) {
  return new Paragraph({
    numbering: { reference: ref, level: 0 },
    spacing: { after: 80, line: 276 },
    children: runs.map(
      (r) =>
        new TextRun({
          text: r.text,
          bold: r.bold || false,
          size: r.size || 22,
          font: "Arial",
          color: r.color || black,
        })
    ),
  });
}

function cellPara(text, opts = {}) {
  const { bold = false, color = black, size = 18, align } = opts;
  return new Paragraph({
    alignment: align,
    spacing: { after: 0, before: 0, line: 260 },
    children: [new TextRun({ text, bold, size, color, font: "Arial" })],
  });
}

function makeCell(text, width, opts = {}) {
  const {
    bold = false,
    fill,
    color = black,
    align,
    size = 18,
    vAlign = VerticalAlign.CENTER,
  } = opts;
  return new TableCell({
    borders,
    width: { size: width, type: WidthType.DXA },
    shading: fill ? { fill, type: ShadingType.CLEAR } : undefined,
    verticalAlign: vAlign,
    margins: { top: 60, bottom: 60, left: 100, right: 100 },
    children: [cellPara(text, { bold, color, size, align })],
  });
}

function makeTable(headers, rows, colWidths) {
  const headerRow = new TableRow({
    tableHeader: true,
    children: headers.map((h, i) =>
      makeCell(h, colWidths[i], {
        bold: true,
        fill: teal,
        color: "FFFFFF",
        size: 18,
      })
    ),
  });

  const dataRows = rows.map(
    (row, ri) =>
      new TableRow({
        children: row.map((c, i) => {
          const isObj = typeof c === "object" && c !== null;
          return makeCell(isObj ? c.text : c, colWidths[i], {
            bold: isObj ? c.bold : false,
            fill: ri % 2 === 1 ? grayHeader : "FFFFFF",
            color: isObj && c.color ? c.color : black,
            size: 18,
          });
        }),
      })
  );

  return new Table({
    width: { size: CONTENT_W, type: WidthType.DXA },
    columnWidths: colWidths,
    rows: [headerRow, ...dataRows],
  });
}

function callout(text) {
  return new Table({
    width: { size: CONTENT_W, type: WidthType.DXA },
    columnWidths: [CONTENT_W],
    rows: [
      new TableRow({
        children: [
          new TableCell({
            borders: {
              top: { style: BorderStyle.SINGLE, size: 4, color: "F59E0B" },
              bottom: { style: BorderStyle.SINGLE, size: 4, color: "F59E0B" },
              left: { style: BorderStyle.SINGLE, size: 24, color: "F59E0B" },
              right: { style: BorderStyle.SINGLE, size: 4, color: "F59E0B" },
            },
            width: { size: CONTENT_W, type: WidthType.DXA },
            shading: { fill: amberSoft, type: ShadingType.CLEAR },
            margins: { top: 100, bottom: 100, left: 140, right: 140 },
            children: [
              new Paragraph({
                spacing: { after: 0, line: 276 },
                children: [
                  new TextRun({
                    text: "Lưu ý: ",
                    bold: true,
                    size: 20,
                    font: "Arial",
                    color: "92400E",
                  }),
                  new TextRun({
                    text,
                    size: 20,
                    font: "Arial",
                    color: "78350F",
                  }),
                ],
              }),
            ],
          }),
        ],
      }),
    ],
  });
}

function spacer(after = 120) {
  return new Paragraph({ spacing: { after }, children: [] });
}

const children = [
  // Title block
  new Paragraph({
    alignment: AlignmentType.CENTER,
    spacing: { after: 80 },
    children: [
      new TextRun({
        text: "HIS XML SYNC",
        bold: true,
        size: 20,
        font: "Arial",
        color: teal,
      }),
    ],
  }),
  new Paragraph({
    alignment: AlignmentType.CENTER,
    spacing: { after: 120 },
    children: [
      new TextRun({
        text: "Hướng dẫn sử dụng",
        bold: true,
        size: 40,
        font: "Arial",
        color: tealDark,
      }),
    ],
  }),
  new Paragraph({
    alignment: AlignmentType.CENTER,
    spacing: { after: 80 },
    children: [
      new TextRun({
        text: "Đồng bộ kết quả đo khúc xạ TOPCON KR-800 lên hệ thống HIS",
        size: 22,
        font: "Arial",
        color: textMuted,
        italics: true,
      }),
    ],
  }),
  new Paragraph({
    alignment: AlignmentType.CENTER,
    spacing: { after: 280 },
    children: [
      new TextRun({
        text: "Dành cho nhân viên vận hành phòng khám  ·  Bản dễ hiểu cho người dùng cuối",
        size: 18,
        font: "Arial",
        color: textMuted,
      }),
    ],
  }),

  // Intro
  richP(
    [
      {
        text: "HIS XML Sync",
        bold: true,
      },
      {
        text: " giúp đưa kết quả đo khúc xạ từ máy ",
      },
      { text: "TOPCON KR-800", bold: true },
      { text: " (file XML) lên hệ thống " },
      { text: "HIS", bold: true },
      { text: " của phòng khám / bệnh viện." },
    ],
    { spacingAfter: 140 }
  ),
  p("Bạn chỉ cần làm 2 việc chính:", { bold: true, spacingAfter: 80 }),
  numberedRich(
    [
      { text: "Cấu hình", bold: true },
      { text: " kết nối HIS (một lần hoặc khi đổi tài khoản)." },
    ],
    "intro-steps"
  ),
  numberedRich(
    [
      { text: "Chọn thư mục", bold: true },
      { text: " chứa file XML và bấm " },
      { text: "Xử lý", bold: true },
      { text: "." },
    ],
    "intro-steps"
  ),

  // Section 1
  h1("1. Giao diện tổng quan"),
  p("Khi mở app, màn hình chia làm 2 phần:", { spacingAfter: 120 }),
  spacer(60),
  makeTable(
    ["Vùng", "Ý nghĩa"],
    [
      ["Menu bên trái", "Chuyển giữa các chức năng"],
      ["Nội dung chính", "Thao tác và xem kết quả"],
    ],
    [3300, CONTENT_W - 3300]
  ),
  spacer(160),

  h2("Menu bên trái"),
  bulletRich([
    { text: "KR-800", bold: true },
    { text: " — Màn hình làm việc hằng ngày: theo dõi file XML và gửi kết quả lên HIS." },
  ]),
  bulletRich([
    { text: "Cấu hình", bold: true },
    {
      text: " — Thiết lập kết nối HIS, xem phiên đăng nhập, xuất nhật ký khi cần hỗ trợ kỹ thuật.",
    },
  ]),

  h2("Góc trên bên phải"),
  bulletRich([
    { text: "Tên cơ sở", bold: true },
    { text: " (ví dụ: Cơ sở HIS demo)." },
  ]),
  bulletRich([
    { text: "Trạng thái đăng nhập HIS: ", bold: false },
    { text: "HIS: chưa login", bold: true },
    { text: " hoặc đã đăng nhập." },
  ]),
  bulletRich([
    { text: "Nút Đổi key", bold: true },
    { text: " — dùng khi cần đổi license key của app." },
  ]),
  spacer(100),
  callout("Muốn gửi dữ liệu lên HIS, app phải đã login HIS thành công."),
  spacer(80),

  // Section 2
  h1("2. Bước 1 — Cấu hình kết nối HIS"),
  richP([
    { text: "Vào menu " },
    { text: "Cấu hình", bold: true },
    { text: " (bên trái)." },
  ]),

  h2("2.1. Điền thông tin kết nối"),
  p("Trong phần Cấu hình kết nối, nhập:", { spacingAfter: 120 }),
  spacer(40),
  makeTable(
    ["Ô nhập", "Điền gì?"],
    [
      ["API URL HIS (base)", "Địa chỉ máy chủ HIS, ví dụ: https://hisvn.vietngagroup.vn"],
      ["Tài khoản (taiKhoan)", "Tài khoản API do bộ phận IT / HIS cấp"],
      ["Mật khẩu (matKhau)", "Mật khẩu tương ứng (hiển thị dạng chấm tròn)"],
      ["Cơ sở khám bệnh ID", "Mã số cơ sở trên HIS (ví dụ: 4)"],
    ],
    [3600, CONTENT_W - 3600]
  ),
  spacer(120),

  h2("2.2. Tùy chọn “Sao chép kết quả khúc xạ sang kính mới”"),
  bulletRich([
    { text: "Bỏ trống (mặc định):", bold: true },
    { text: " chỉ gửi kết quả đo khúc xạ theo quy trình chuẩn." },
  ]),
  bulletRich([
    { text: "Tích chọn:", bold: true },
    {
      text: " app sẽ sao chép kết quả khúc xạ sang phần kính mới trên HIS (chỉ bật khi phòng khám yêu cầu).",
    },
  ]),

  h2("2.3. Lưu và đăng nhập"),
  p("Có 2 nút:", { spacingAfter: 100 }),
  spacer(40),
  makeTable(
    ["Nút", "Khi nào dùng"],
    [
      [
        "Kiểm tra / Login HIS",
        "Thử đăng nhập để xem thông tin đã đúng chưa (chưa bắt buộc lưu).",
      ],
      [
        { text: "Lưu & Login", bold: true },
        "Nên dùng hằng ngày / lần đầu: lưu cấu hình rồi đăng nhập HIS.",
      ],
    ],
    [3600, CONTENT_W - 3600]
  ),
  spacer(140),
  p("Sau khi thành công:", { bold: true, spacingAfter: 80 }),
  bullet("Góc phải hiển thị trạng thái đã kiểm tra / đã có phiên đăng nhập."),
  bullet("Phần Phiên đăng nhập HIS hiện “Đã login”, tên user, cơ sở, thời hạn token, v.v."),
  spacer(80),
  p("Nếu “Chưa login” hoặc báo lỗi:", { bold: true, spacingAfter: 80 }),
  numbered("Kiểm tra lại URL, tài khoản, mật khẩu, ID cơ sở.", "login-fix"),
  numbered("Kiểm tra máy có vào được mạng nội bộ / HIS không.", "login-fix"),
  numbered("Bấm lại Lưu & Login.", "login-fix"),

  h2("2.4. Phiên đăng nhập HIS (chỉ để xem)"),
  p("Khu vực này cho biết app đang đăng nhập HIS hay chưa:", { spacingAfter: 80 }),
  bulletRich([{ text: "Trạng thái:", bold: true }, { text: " Chưa login / Đã login" }]),
  bulletRich([{ text: "User:", bold: true }, { text: " người dùng API" }]),
  bulletRich([{ text: "coSoKcbId:", bold: true }, { text: " cơ sở đang dùng" }]),
  bulletRich([
    { text: "Hết hạn token:", bold: true },
    { text: " thời điểm phiên đăng nhập hết hạn" },
  ]),
  bulletRich([{ text: "Có access_token:", bold: true }, { text: " Có / Không" }]),
  spacer(60),
  richP([
    { text: "Bạn không cần nhớ token. ", bold: true },
    {
      text: "App tự quản lý; token không hiện đầy đủ trên màn hình để bảo mật.",
    },
  ]),

  h2("2.5. Nhật ký ứng dụng (khi cần hỗ trợ)"),
  p(
    "Phần Nhật ký ứng dụng ghi lại các request/response (không lưu mật khẩu / token đầy đủ) để kỹ thuật viên kiểm tra lỗi.",
    { spacingAfter: 80 }
  ),
  bulletRich([
    { text: "Bấm Xuất logs", bold: true },
    { text: " khi IT / hỗ trợ yêu cầu gửi file log." },
  ]),
  bullet("Có thể xem đường dẫn file log và dung lượng file."),

  // Section 3
  h1("3. Bước 2 — Xử lý file XML từ máy KR-800"),
  richP([
    { text: "Vào menu " },
    { text: "KR-800", bold: true },
    { text: ". Đây là màn hình làm việc chính sau khi đã cấu hình và login HIS." },
  ]),

  h2("3.1. Chọn thư mục tracking"),
  numbered("Bấm Chọn thư mục.", "pick-folder"),
  numbered(
    "Chọn thư mục máy TOPCON KR-800 xuất file XML (ví dụ: Tool_Send_KQ_ToHis).",
    "pick-folder"
  ),
  numbered("App sẽ quét các file .xml trong thư mục đó.", "pick-folder"),
  spacer(60),
  p("Sau khi chọn xong:", { bold: true, spacingAfter: 80 }),
  bullet("Đường dẫn thư mục hiển thị ở Thư mục tracking."),
  bullet("Các chip thống kê: Trong khoảng, Chờ, Lỗi, …"),
  spacer(60),
  richP([
    { text: "Quét lại:", bold: true },
    {
      text: " dùng khi máy vừa xuất thêm file XML mới mà danh sách chưa cập nhật.",
    },
  ]),

  h2("3.2. Chọn khoảng thời gian xử lý"),
  p("Ở dòng Ngày xử lý:", { spacingAfter: 80 }),
  bullet("Chọn Từ ngày/giờ → Đến ngày/giờ."),
  bullet("Danh sách file chỉ hiện những file có ngày tạo nằm trong khoảng này."),
  p("Ví dụ: từ 06/07/2026 00:00 đến 10/07/2026 23:59.", {
    italic: true,
    color: textMuted,
    spacingBefore: 40,
  }),
  spacer(60),
  callout(
    "Nếu bảng trống nhưng bạn biết có file: mở rộng khoảng ngày, hoặc bấm Quét lại."
  ),
  spacer(80),

  h2("3.3. Xem danh sách file XML"),
  p("Bảng gồm:", { spacingAfter: 100 }),
  spacer(40),
  makeTable(
    ["Cột", "Ý nghĩa"],
    [
      ["Tên file", "Tên file XML từ máy đo"],
      ["Kích thước", "Dung lượng file"],
      ["Trạng thái", "File đang ở bước nào (xem mục 4)"],
      ["Ngày tạo", "Thời điểm file được tạo"],
      ["Ngày cập nhật", "Lần app cập nhật trạng thái gần nhất"],
      ["Lỗi", "Chi tiết lỗi (nếu có)"],
    ],
    [3000, CONTENT_W - 3000]
  ),
  spacer(120),

  h2("3.4. Bấm “Xử lý”"),
  numbered(
    "Đảm bảo đã login HIS (góc trên phải không còn “chưa login”).",
    "process-steps"
  ),
  numbered("Đã chọn thư mục tracking.", "process-steps"),
  numbered("Đã chọn đúng khoảng Ngày xử lý.", "process-steps"),
  numbered("Bấm nút xanh Xử lý.", "process-steps"),
  spacer(80),
  p("App sẽ lần lượt:", { bold: true, spacingAfter: 80 }),
  numbered("Đọc file XML.", "app-flow"),
  numbered("Tìm bệnh nhân / đợt điều trị tương ứng trên HIS.", "app-flow"),
  numbered("Mapping danh mục và gửi kết quả lên HIS.", "app-flow"),
  spacer(80),
  richP([
    { text: "Trong lúc chạy, nút hiển thị " },
    { text: "Đang xử lý…", bold: true },
    { text: " — vui lòng chờ, không tắt app." },
  ]),
  p("Khi xong, app báo tóm tắt:", { spacingBefore: 40, spacingAfter: 80 }),
  bullet("Đã xử lý bao nhiêu file"),
  bullet("Bỏ qua file trùng"),
  bullet("Bao nhiêu file lỗi"),

  // Section 4
  h1("4. Ý nghĩa các trạng thái file"),
  spacer(40),
  makeTable(
    ["Trạng thái trên màn hình", "Nghĩa đơn giản"],
    [
      ["Chờ xử lý", "File mới, chưa gửi lên HIS"],
      ["Đang xử lý", "App đang làm việc với file này"],
      ["Đã phân tích XML", "Đã đọc được nội dung file"],
      ["Đã tìm thấy bệnh nhân", "Khớp được bệnh nhân trên HIS"],
      ["Đã mapping danh mục", "Đã map dữ liệu đo sang mã HIS"],
      ["Đang gửi HIS", "Đang đẩy kết quả lên server"],
      [
        { text: "Đã xử lý", bold: true, color: "047857" },
        { text: "Thành công — kết quả đã lên HIS", bold: true, color: "047857" },
      ],
      ["Không tìm thấy bệnh nhân", "Mã BN / thông tin trong XML không khớp HIS"],
      [
        "Không xác định đợt điều trị",
        "Tìm thấy BN nhưng không rõ đợt khám / điều trị",
      ],
      ["Lỗi XML", "File hỏng, sai định dạng, hoặc đọc không được"],
      ["Lỗi mapping danh mục", "Không map được chỉ số đo sang danh mục HIS"],
      ["Lỗi gửi HIS", "Mạng / API HIS từ chối hoặc lỗi khi gửi"],
      ["Thất bại", "Xử lý không thành công (xem cột Lỗi)"],
    ],
    [4000, CONTENT_W - 4000]
  ),
  spacer(140),
  richP([
    { text: "Mục tiêu mong muốn: ", bold: true },
    { text: "file chuyển sang " },
    { text: "Đã xử lý", bold: true, color: "047857" },
    { text: ", cột Lỗi trống." },
  ]),

  // Section 5
  h1("5. Quy trình làm việc gợi ý mỗi ngày"),
  numbered("Mở app HIS XML Sync.", "daily"),
  numberedRich(
    [
      { text: "Kiểm tra góc trên phải: đã login HIS chưa." },
      { text: " Nếu chưa → Cấu hình → Lưu & Login.", bold: true },
    ],
    "daily"
  ),
  numbered("Vào KR-800.", "daily"),
  numbered("Chọn / kiểm tra đúng thư mục XML của máy đo.", "daily"),
  numbered("Bấm Quét lại (nếu vừa đo xong).", "daily"),
  numbered("Chọn khoảng Ngày xử lý phù hợp (thường là hôm nay).", "daily"),
  numbered("Bấm Xử lý.", "daily"),
  numbered(
    "Xem cột Trạng thái: Đã xử lý → OK; Lỗi / Không tìm thấy bệnh nhân → xử lý theo mục 6.",
    "daily"
  ),

  // Section 6
  h1("6. Xử lý sự cố thường gặp"),

  h2("“HIS: chưa login”"),
  bullet("Vào Cấu hình → kiểm tra URL, tài khoản, mật khẩu, ID cơ sở."),
  bullet("Bấm Lưu & Login."),
  bullet("Kiểm tra mạng nội bộ / VPN nếu HIS chỉ mở trong viện."),

  h2("Bảng file trống"),
  bullet("Đã Chọn thư mục đúng chỗ máy xuất XML chưa?"),
  bullet("Bấm Quét lại."),
  bullet("Mở rộng Ngày xử lý (từ–đến)."),
  bullet("Kiểm tra thư mục có file .xml thật không (mở Explorer / Finder)."),

  h2("File “Không tìm thấy bệnh nhân”"),
  bullet("Kiểm tra bệnh nhân đã có trên HIS chưa."),
  bullet("Mã bệnh nhân trong XML (thường gắn theo quy trình đo) có đúng với HIS không."),
  bullet("Khoảng Ngày xử lý có bao phủ thời điểm bệnh nhân vào viện / khám không."),

  h2("File “Lỗi gửi HIS” hoặc “Thất bại”"),
  bullet("Xem cột Lỗi trên bảng."),
  bullet("Thử Lưu & Login lại rồi Xử lý lại."),
  bullet("Nếu vẫn lỗi: Cấu hình → Xuất logs → gửi file cho IT / hỗ trợ kỹ thuật."),

  h2("Nút “Xử lý” bị mờ, không bấm được"),
  p("Có thể do:", { spacingAfter: 80 }),
  bullet("Chưa chọn thư mục tracking"),
  bullet("Đang quét / đang tải danh sách"),
  bullet("Đang xử lý lượt trước"),
  bullet("Khoảng thời gian “Từ” > “Đến” (không hợp lệ)"),
  bullet("App đang bận thao tác login HIS khác"),
  p("Sửa điều kiện trên rồi thử lại.", { spacingBefore: 40 }),

  // Section 7
  h1("7. Lưu ý an toàn & vận hành"),
  bulletRich([
    { text: "Không chia sẻ", bold: true },
    { text: " mật khẩu HIS và license key." },
  ]),
  bullet("Chỉ cấu hình trên máy làm việc của phòng đo / IT được giao."),
  bullet(
    "App lưu cấu hình cục bộ; khi đổi mật khẩu HIS, vào Cấu hình và Lưu & Login lại."
  ),
  bullet(
    "Nên để app mở trong ca làm việc nếu thường xuyên có file XML mới; sau mỗi đợt đo, Quét lại → Xử lý."
  ),

  // Section 8
  h1("8. Tóm tắt nhanh (1 phút)"),
  spacer(40),
  makeTable(
    ["Việc cần làm", "Ở đâu", "Nút chính"],
    [
      ["Kết nối HIS lần đầu / đổi TK", "Cấu hình", "Lưu & Login"],
      ["Chọn chỗ chứa file XML", "KR-800", "Chọn thư mục"],
      ["Cập nhật file mới", "KR-800", "Quét lại"],
      ["Gửi kết quả lên HIS", "KR-800", "Xử lý"],
      ["Gửi log khi lỗi", "Cấu hình", "Xuất logs"],
    ],
    [4200, 2400, CONTENT_W - 6600]
  ),
  spacer(240),

  new Paragraph({
    alignment: AlignmentType.CENTER,
    spacing: { before: 200 },
    border: {
      top: { style: BorderStyle.SINGLE, size: 6, color: grayBorder, space: 12 },
    },
    children: [
      new TextRun({
        text: "Tài liệu dành cho nhân viên vận hành phòng khám. ",
        size: 18,
        italics: true,
        font: "Arial",
        color: textMuted,
      }),
    ],
  }),
  new Paragraph({
    alignment: AlignmentType.CENTER,
    spacing: { after: 80 },
    children: [
      new TextRun({
        text: "Nếu cần cấu hình server, license hoặc cài đặt máy, liên hệ bộ phận IT / nhà cung cấp.",
        size: 18,
        italics: true,
        font: "Arial",
        color: textMuted,
      }),
    ],
  }),
];

const doc = new Document({
  styles: {
    default: {
      document: {
        run: { font: "Arial", size: 22 },
      },
    },
    paragraphStyles: [
      {
        id: "Heading1",
        name: "Heading 1",
        basedOn: "Normal",
        next: "Normal",
        quickFormat: true,
        run: { size: 28, bold: true, font: "Arial", color: tealDark },
        paragraph: { spacing: { before: 320, after: 160 }, outlineLevel: 0 },
      },
      {
        id: "Heading2",
        name: "Heading 2",
        basedOn: "Normal",
        next: "Normal",
        quickFormat: true,
        run: { size: 24, bold: true, font: "Arial", color: teal },
        paragraph: { spacing: { before: 240, after: 120 }, outlineLevel: 1 },
      },
    ],
  },
  numbering: {
    config: [
      {
        reference: "bullets",
        levels: [
          {
            level: 0,
            format: LevelFormat.BULLET,
            text: "•",
            alignment: AlignmentType.LEFT,
            style: { paragraph: { indent: { left: 720, hanging: 360 } } },
          },
        ],
      },
      {
        reference: "intro-steps",
        levels: [
          {
            level: 0,
            format: LevelFormat.DECIMAL,
            text: "%1.",
            alignment: AlignmentType.LEFT,
            style: { paragraph: { indent: { left: 720, hanging: 360 } } },
          },
        ],
      },
      {
        reference: "login-fix",
        levels: [
          {
            level: 0,
            format: LevelFormat.DECIMAL,
            text: "%1.",
            alignment: AlignmentType.LEFT,
            style: { paragraph: { indent: { left: 720, hanging: 360 } } },
          },
        ],
      },
      {
        reference: "pick-folder",
        levels: [
          {
            level: 0,
            format: LevelFormat.DECIMAL,
            text: "%1.",
            alignment: AlignmentType.LEFT,
            style: { paragraph: { indent: { left: 720, hanging: 360 } } },
          },
        ],
      },
      {
        reference: "process-steps",
        levels: [
          {
            level: 0,
            format: LevelFormat.DECIMAL,
            text: "%1.",
            alignment: AlignmentType.LEFT,
            style: { paragraph: { indent: { left: 720, hanging: 360 } } },
          },
        ],
      },
      {
        reference: "app-flow",
        levels: [
          {
            level: 0,
            format: LevelFormat.DECIMAL,
            text: "%1.",
            alignment: AlignmentType.LEFT,
            style: { paragraph: { indent: { left: 720, hanging: 360 } } },
          },
        ],
      },
      {
        reference: "daily",
        levels: [
          {
            level: 0,
            format: LevelFormat.DECIMAL,
            text: "%1.",
            alignment: AlignmentType.LEFT,
            style: { paragraph: { indent: { left: 720, hanging: 360 } } },
          },
        ],
      },
    ],
  },
  sections: [
    {
      properties: {
        page: {
          size: { width: PAGE_W, height: PAGE_H },
          margin: {
            top: MARGIN,
            right: MARGIN,
            bottom: MARGIN + 200,
            left: MARGIN,
          },
        },
      },
      headers: {
        default: new Header({
          children: [
            new Paragraph({
              alignment: AlignmentType.RIGHT,
              border: {
                bottom: {
                  style: BorderStyle.SINGLE,
                  size: 6,
                  color: teal,
                  space: 6,
                },
              },
              spacing: { after: 120 },
              children: [
                new TextRun({
                  text: "HIS XML Sync  ·  Hướng dẫn sử dụng",
                  size: 16,
                  font: "Arial",
                  color: teal,
                }),
              ],
            }),
          ],
        }),
      },
      footers: {
        default: new Footer({
          children: [
            new Paragraph({
              alignment: AlignmentType.CENTER,
              border: {
                top: {
                  style: BorderStyle.SINGLE,
                  size: 4,
                  color: grayBorder,
                  space: 8,
                },
              },
              spacing: { before: 80 },
              children: [
                new TextRun({
                  text: "Trang ",
                  size: 16,
                  font: "Arial",
                  color: textMuted,
                }),
                new TextRun({
                  children: [PageNumber.CURRENT],
                  size: 16,
                  font: "Arial",
                  color: textMuted,
                }),
                new TextRun({
                  text: " / ",
                  size: 16,
                  font: "Arial",
                  color: textMuted,
                }),
                new TextRun({
                  children: [PageNumber.TOTAL_PAGES],
                  size: 16,
                  font: "Arial",
                  color: textMuted,
                }),
              ],
            }),
          ],
        }),
      },
      children,
    },
  ],
});

const outDir = __dirname;
const outDocx = path.join(outDir, "huong-dan-su-dung.docx");

Packer.toBuffer(doc).then((buffer) => {
  fs.writeFileSync(outDocx, buffer);
  console.log("Wrote:", outDocx);
});

import java.sql.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

// Java 11 source launcher. Credentials are received on stdin, never argv.
class NavkitH2 {
  static Connection conn;
  static String initialSchema;
  static String decode(String s) { return new String(Base64.getDecoder().decode(s), StandardCharsets.UTF_8); }
  static String json(String s) {
    if (s == null) return "null";
    StringBuilder b = new StringBuilder("\"");
    for (char c : s.toCharArray()) {
      switch(c) { case '"': b.append("\\\""); break; case '\\': b.append("\\\\"); break;
        case '\n': b.append("\\n"); break; case '\r': b.append("\\r"); break; case '\t': b.append("\\t"); break;
        default: if(c < 32) b.append(String.format("\\u%04x", (int)c)); else b.append(c); }
    }
    return b.append('"').toString();
  }
  public static void main(String[] args) throws Exception {
    Class.forName("org.h2.Driver");
    BufferedReader in = new BufferedReader(new InputStreamReader(System.in, StandardCharsets.UTF_8));
    PrintWriter out = new PrintWriter(new OutputStreamWriter(System.out, StandardCharsets.UTF_8), true);
    String line;
    while ((line = in.readLine()) != null) {
      try {
        String[] p = line.split("\t", -1);
        if (p[0].equals("connect")) {
          conn = DriverManager.getConnection(decode(p[1]), decode(p[2]), decode(p[3]));
          initialSchema = conn.getSchema();
          out.println("{\"columns\":[\"version\"],\"rows\":[[" + json(conn.getMetaData().getDatabaseProductVersion()) + "]]}");
        } else if (p[0].equals("query")) {
          String requestedSchema = p.length > 3 ? decode(p[3]) : "";
          conn.setSchema(requestedSchema.isEmpty() ? initialSchema : requestedSchema);
          try (Statement st = conn.createStatement()) {
            st.setQueryTimeout(Integer.parseInt(p[2]));
            st.setMaxRows(10001);
            boolean has = st.execute(decode(p[1]));
            if (!has) { if (conn.isClosed()) { out.println("{\"columns\":[\"affected\"],\"rows\":[[\"0\"]]}"); continue; } out.println("{\"columns\":[\"affected\"],\"rows\":[[" + json(Long.toString(st.getLargeUpdateCount())) + "]]}"); continue; }
            try (ResultSet rs = st.getResultSet()) {
              ResultSetMetaData m = rs.getMetaData(); int n = m.getColumnCount();
              StringBuilder b = new StringBuilder("{\"columns\":[");
              for(int i=1;i<=n;i++) { if(i>1)b.append(','); b.append(json(m.getColumnLabel(i))); }
              b.append("],\"rows\":["); boolean first=true; int returned=0;
              while(rs.next()) { if(++returned>10000) throw new SQLException("H2 query exceeds 10000-row safety limit; use explicit LIMIT/paging", "54000"); if(!first)b.append(','); first=false; b.append('[');
                for(int i=1;i<=n;i++) { if(i>1)b.append(','); b.append(json(rs.getString(i))); } b.append(']'); }
              out.println(b.append("]}").toString());
            }
          }
        } else throw new IllegalArgumentException("Unknown sidecar command");
      } catch (Exception e) {
        String state = e instanceof SQLException ? ((SQLException)e).getSQLState() : "";
        out.println("{\"error\":" + json(e.getClass().getSimpleName()+" ["+state+"]: "+e.getMessage()) + "}");
      }
    }
    if(conn!=null) conn.close();
  }
}

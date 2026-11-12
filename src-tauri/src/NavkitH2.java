import java.sql.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

// Java 11 source launcher. Credentials are received on stdin, never argv.
class NavkitH2 {
  static Connection conn;
  static JdbcMutationSupport relational;
  static String initialSchema, password="";
  static String decode(String value) { return new String(Base64.getDecoder().decode(value), StandardCharsets.UTF_8); }
  static String json(String value) { return value==null?"null":NavkitJsonString.quote(value); }
  static String cell(ResultSet rows,ResultSetMetaData metadata,int position) throws SQLException {
    switch(metadata.getColumnType(position)) {
      case Types.BINARY:case Types.VARBINARY:case Types.LONGVARBINARY:case Types.BLOB:
        byte[] bytes=rows.getBytes(position);return bytes==null?null:"base64:"+Base64.getEncoder().encodeToString(bytes);
      default:return rows.getString(position);
    }
  }
  static void query(PrintWriter output,String[] parts) throws SQLException {
    String requested=parts.length>3?decode(parts[3]):"";
    String selected=requested.isEmpty()?initialSchema:requested;
    conn.setSchema(selected);
    if(!Objects.equals(conn.getSchema(),selected))throw new SQLException("H2 did not select requested schema", "3F000");
    try(Statement statement=conn.createStatement()) {
      statement.setQueryTimeout(Integer.parseInt(parts[2]));
      statement.setMaxRows(10001);
      boolean has=statement.execute(decode(parts[1]));
      if(!has) {
        // Legacy scalar response only; never used as C2 mutation evidence.
        long affected=conn.isClosed()?0:statement.getLargeUpdateCount();
        output.println("{\"columns\":[\"affected\"],\"rows\":[["+json(Long.toString(affected))+"]]}");
        return;
      }
      try(ResultSet rows=statement.getResultSet()) {
        ResultSetMetaData metadata=rows.getMetaData();int count=metadata.getColumnCount();
        StringBuilder out=new StringBuilder("{\"columns\":[");
        for(int i=1;i<=count;i++) { if(i>1)out.append(',');out.append(json(metadata.getColumnLabel(i))); }
        out.append("],\"rows\":[");boolean first=true;int returned=0;
        while(rows.next()) {
          if(++returned>10000)throw new SQLException("H2 query exceeds 10000-row safety limit; use explicit LIMIT/paging", "54000");
          if(!first)out.append(',');first=false;out.append('[');
          for(int i=1;i<=count;i++) { if(i>1)out.append(',');out.append(json(cell(rows,metadata,i))); }
          out.append(']');
        }
        output.println(out.append("]}").toString());
      }
    }
  }
  public static void main(String[] args) throws Exception {
    Class.forName("org.h2.Driver");
    BufferedReader input=new BufferedReader(new InputStreamReader(System.in,StandardCharsets.UTF_8));
    PrintWriter output=new PrintWriter(new OutputStreamWriter(System.out,StandardCharsets.UTF_8),true);
    try {
      String line;
      while((line=JdbcMutationSupport.readLine(input))!=null) {
        try {
          String[] parts=line.split("\t",-1);
          if(parts[0].equals("connect")) {
            if(conn!=null)throw new SQLException("H2 sidecar already owns a physical connection", "08002");
            password=decode(parts[3]);
            conn=DriverManager.getConnection(decode(parts[1]),decode(parts[2]),password);
            initialSchema=conn.getSchema();relational=new JdbcMutationSupport(conn,"native-h2");
            output.println("{\"columns\":[\"version\"],\"rows\":[["+json(conn.getMetaData().getDatabaseProductVersion())+"]]}");
          } else if(parts[0].equals("v2")) {
            if(relational==null || parts.length!=2)throw new SQLException("H2 relational sidecar is not connected", "08003");
            relational.handle(parts[1],output,password);
          } else if(parts[0].equals("query"))query(output,parts);
          else throw new IllegalArgumentException("Unknown sidecar command");
        } catch(Exception error) {
          String state=error instanceof SQLException?((SQLException)error).getSQLState():"";
          String detail=error.getClass().getSimpleName()+" ["+state+"]: "+error.getMessage();
          if(!password.isEmpty())detail=detail.replace(password,"[REDACTED]");
          output.println("{\"error\":"+json(detail)+"}");
        }
        if(output.checkError())break;
      }
    } finally { if(conn!=null && !conn.isClosed())conn.close(); }
  }
}

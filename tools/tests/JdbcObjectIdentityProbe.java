import java.sql.*;
import java.util.*;

class JdbcObjectIdentityProbe {
  static String scalar(Connection c,String sql) throws Exception { try(Statement s=c.createStatement();ResultSet r=s.executeQuery(sql)) { r.next();return r.getString(1); } }
  static Object table(Connection c) throws Exception {
    Class<?> jdbc=Class.forName("org.h2.jdbc.JdbcConnection");
    Object actual=c.unwrap(jdbc), session=jdbc.getMethod("getSession").invoke(actual);
    System.out.println("SESSION_CLASS="+session.getClass().getName());
    Object database=session.getClass().getMethod("getDatabase").invoke(session);
    Object schema=database.getClass().getMethod("findSchema",String.class).invoke(database,"PUBLIC");
    return schema.getClass().getMethod("findTableOrView",Class.forName("org.h2.engine.SessionLocal"),String.class).invoke(schema,session,"T");
  }
  public static void main(String[] args) throws Exception {
    try(Connection c=DriverManager.getConnection("jdbc:h2:mem:pl20_identity_"+UUID.randomUUID().toString().replace("-",""),"sa","")) {
      try(Statement s=c.createStatement()) { s.execute("CREATE TABLE T(ID BIGINT PRIMARY KEY,PAY VARCHAR(100))"); }
      String before=scalar(c,"SELECT DB_OBJECT_ID('TABLE','PUBLIC','T')"), ddl=scalar(c,"SELECT DB_OBJECT_SQL('TABLE','PUBLIC','T')");Object original=table(c);
      try(Statement s=c.createStatement()) { s.execute("DROP TABLE T");s.execute("CREATE TABLE T(ID BIGINT PRIMARY KEY,PAY VARCHAR(100))"); }
      String after=scalar(c,"SELECT DB_OBJECT_ID('TABLE','PUBLIC','T')"), nextDdl=scalar(c,"SELECT DB_OBJECT_SQL('TABLE','PUBLIC','T')");Object replacement=table(c);
      System.out.println("BEFORE_ID="+before+" AFTER_ID="+after+" SAME_DDL="+ddl.equals(nextDdl)+" SAME_PHYSICAL_TABLE="+(original==replacement));
      if(replacement==null || original==null || replacement==original)throw new AssertionError("Actual H2 object reflection must identify replacement table");
    }
  }
}
